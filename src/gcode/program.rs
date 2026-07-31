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

use crate::gcode::arcfit;
use crate::gcode::coder::{Coder, ProgramPrimitives};
use crate::gcode::feeds::{self, FeedsError, FeedsSpeeds, MachineLimits, Motion};
use crate::gcode::plan::{OpKind, Point, StepPlan};
use crate::gcode::routing::{self, RouteMove};
use crate::gcode::step_data::StepValue;

/// The per-step rendering context the body needs beyond the plan: the CNC's operation
/// primitive templates, its spindle and axis limits (for the feed/speed solve), and
/// whether tool changes are automatic (ATC) — a manual machine gets an operator prompt
/// instead.
#[derive(Clone)]
pub struct StepRender {
    pub drill_tpl: String,
    pub tool_change_tpl: String,
    /// Emitted between `tool_change` and `spindle_start`, and only when
    /// [`Self::measures_tool_length`] — see there.
    pub tool_measure_tpl: String,
    pub spindle_start_tpl: String,
    pub spindle_stop_tpl: String,
    /// Motion primitives used by routed holes (spiral pocket): rapid to position, feed
    /// cut (plunge + radial), and the arc for each full circle.
    pub move_rapid_tpl: String,
    pub cut_linear_tpl: String,
    pub cut_arc_tpl: String,
    /// How far the emitted path may depart from the true curve
    /// (`machine.curve_tolerance`). Governs arc fitting and arc flattening alike, so one
    /// profile cannot be accurate in one and coarse in the other.
    pub curve_tolerance: units::Length,
    pub limits: MachineLimits,
    pub is_atc: bool,
    /// Whether this machine needs the tool measured after each change
    /// (`machine.tool_length_measurement == manual`). A machine with an automatic setter
    /// measures at M06, so emitting a block for it would be a second, redundant cycle.
    pub measures_tool_length: bool,
    /// An operator prompt emitted **before the first tool block**, if any.
    ///
    /// Set for a back-face step, and it is the only guard that exists against the board
    /// being remounted the wrong way up: locating pins are symmetric and the same
    /// diameter, so the board drops onto them just as happily unflipped or turned 180°.
    /// Nothing in the geometry can reject that — only the operator can, and only if they
    /// are asked.
    ///
    /// Emitted through the machine's own `pause` primitive, so a controller with no word
    /// for a pause emits nothing at all. The planner warns when that is the case, because
    /// a missing guard is worth knowing about before the board is in the fixture.
    pub opening_prompt: Option<String>,
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
    pub program_begin_tpl: String,
    pub program_end_tpl: String,
    /// The CNC's `line_format` filter, applied to every line of the finished program.
    /// Empty leaves the program exactly as the generators built it.
    pub line_format_tpl: String,
    /// The CNC's `set_unit` primitive — what `metric()`/`imperial()` emit. Empty for a
    /// machine with no unit statement.
    pub set_unit_tpl: String,
    /// The CNC's `set_origin` primitive — what `set_origin()` emits, and what validates
    /// [`Self::origin_reference`]. Empty for a machine that selects no origin.
    pub set_origin_tpl: String,
    /// The operator callables: what `comment("…")`, `message("…")` and `pause("…")` emit
    /// when a template calls them. Empty means the machine has no word for it, and the
    /// call emits nothing.
    pub comment_tpl: String,
    pub message_tpl: String,
    pub pause_tpl: String,
    /// This step's fixture's safe travel height — the header and footer retract to it.
    pub z_safe: units::Length,
    /// Which of the machine's stored zeros this step's fixture sits in, named the way this
    /// machine names it, exactly as the operator entered it.
    pub origin_reference: String,
    /// The extension a saved program takes, from the CNC profile — so an Excellon step
    /// is not written as `.nc`.
    pub file_extension: String,
    pub body: StepRender,
}

/// What every step's header and footer sees, identical across the whole job.
///
/// Gathered into one type rather than passed as loose arguments because these are the
/// values that describe the *job* — as against [`ProgramRender`], which describes the
/// *machine* this step runs on. A header reads from both, and the distinction is what
/// keeps it clear which of the two a new value belongs on.
pub struct ProgramContext<'a> {
    /// The source KiCad board's file name.
    pub filename: &'a str,
    /// When the program was generated, by the local clock.
    pub timestamp: &'a str,
    /// Every step of the machining profile, in order, as the operator configured them.
    /// `steps[step_index]` is the one being rendered.
    pub steps: &'a [StepValue],
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
    ctx: &ProgramContext,
    tool_feeds: &BTreeMap<String, ToolFeed>,
) -> Result<String, BodyError> {
    // Builds the Coder, which renders `set_unit` and `set_origin` up front — so a fixture
    // whose origin reference this machine does not have fails here, before a single line
    // of the program exists.
    let coder = Coder::with_program_primitives(&ProgramPrimitives {
        set_unit: &render.set_unit_tpl,
        set_origin: &render.set_origin_tpl,
        origin_reference: &render.origin_reference,
        comment: &render.comment_tpl,
        message: &render.message_tpl,
        pause: &render.pause_tpl,
    })?;

    // Converted once and cloned per scope: the array is the same for the header and the
    // footer, and building it is the only part of the scope that walks a tree.
    let steps = crate::gcode::step_data::to_array(ctx.steps);

    // `program_begin` and `program_end` see the same values: a footer typically retracts
    // to `z_safe`, and either may echo the source file. A fresh scope for each, but the
    // Coder's unit mode persists between them.
    //
    // `step_index` is the plan's own step number, which is the enumeration of the
    // profile's `/steps` array — so `steps[step_index]` is this step's record and no
    // second index has to be kept in step with the first.
    // Read straight from the crate rather than threaded through [`ProgramContext`]: it
    // describes the *build*, not the job, and is fixed at compile time. `Cargo.toml` is
    // the single source of truth for it, and `build.rs` warns when that falls behind the
    // newest release tag.
    let program_scope = || {
        let mut scope = Scope::new();
        scope.push("k2g_version", env!("CARGO_PKG_VERSION").to_string());
        scope.push("filename", ctx.filename.to_string());
        scope.push("timestamp", ctx.timestamp.to_string());
        scope.push("z_safe", render.z_safe);
        scope.push("origin_reference", render.origin_reference.clone());
        scope.push("steps", steps.clone());
        scope.push("step_index", step.index as i64);
        scope
    };

    let mut scope = program_scope();
    let header = coder
        .render("program_begin", &render.program_begin_tpl, &mut scope)
        .map_err(|err| BodyError::Render { primitive: "program_begin".into(), message: err.to_string() })?;

    let body = render_step_body(&coder, step, &render.body, tool_feeds)?;

    let mut scope = program_scope();
    let footer = coder
        .render("program_end", &render.program_end_tpl, &mut scope)
        .map_err(|err| BodyError::Render { primitive: "program_end".into(), message: err.to_string() })?;

    // Joined with single newlines (trailing ones trimmed) so an empty or multi-line body
    // never introduces stray blank lines.
    let assembled = [header, body, footer]
        .iter()
        .map(|section| section.trim_end_matches('\n'))
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    format_lines(&coder, &assembled, &render.line_format_tpl)
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

    // Before anything moves, and before the first tool change, so the operator is asked
    // while the spindle is still parked and the board is still in their hands.
    if let Some(prompt) = render.opening_prompt.as_deref() {
        let mut scope = Scope::new();
        scope.push("prompt", prompt.to_string());
        out.push_str(&render_one(coder, "pause", OPENING_PROMPT_TPL, &mut scope)?);
    }

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

        // Tool-block boundary: tool_change, then tool_measure on a machine that needs it,
        // then spindle_start (§7). tool_change leads with M05, so it also stops the
        // previous block's spindle.
        let manual_message = if render.is_atc {
            String::new()
        } else {
            format!("(load tool T{slot}: {name})")
        };
        let mut scope = Scope::new();
        scope.push("manual_message", manual_message);
        scope.push("slot", slot);
        scope.push("rpm", fs.rpm);
        out.push_str(&render_one(coder, "tool_change", &render.tool_change_tpl, &mut scope)?);

        // The tool has to be measured before it cuts, and after it is in the spindle —
        // which is exactly here. Skipped on a machine with an automatic setter: that
        // measures at M06, so a block here would be a second, redundant cycle.
        if render.measures_tool_length {
            let mut scope = Scope::new();
            scope.push("slot", slot);
            scope.push("tool_name", name.clone());
            scope.push("diameter", block.diameter);
            out.push_str(&render_one(coder, "tool_measure", &render.tool_measure_tpl, &mut scope)?);
        }

        let mut scope = Scope::new();
        scope.push("rpm", fs.rpm);
        out.push_str(&render_one(coder, "spindle_start", &render.spindle_start_tpl, &mut scope)?);

        // A hole's place in this block's run of holes, so a profile can open a modal cycle
        // on the first and cancel it on the last. Counted over drill ops only: a block that
        // also routes must not have its cycle "cancelled" by a routing op in between.
        let drill_count = block.ops.iter().filter(|op| matches!(op.kind, OpKind::Drill)).count();
        let mut drill_index = 0usize;

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
                    scope.push("index", drill_index as i64);
                    scope.push("count", drill_count as i64);
                    drill_index += 1;
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
                    out.push_str(&render_moves(coder, render, moves, op.entry, fs)?);
                }
                OpKind::RouteContour { ref path } => {
                    // The span carries its own geometry: drop in at its start, feed
                    // through it, lift off at its end. The gap to the next span is the
                    // retaining tab, so the retract between them is not optional.
                    //
                    // The path arrives as a polyline because the routing offset is a
                    // polygon operation (`pcb::routing_offset`), so a curved board edge
                    // comes through as hundreds of chords. Fitting arcs back to it here —
                    // after tab splitting, so no arc ever has to be cut in half — is what
                    // turns those back into `G2`/`G3`. A run that no arc describes within
                    // tolerance stays exactly the chords it already was.
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
                    let mut here = path[0];
                    for seg in arcfit::fit(path, render.curve_tolerance) {
                        match seg {
                            arcfit::PathSeg::Line { to } => {
                                moves.push(RouteMove::Cut { x: to.x, y: to.y, z: op.z.z_bottom });
                            }
                            arcfit::PathSeg::Arc { to, centre, ccw } => {
                                moves.push(RouteMove::Arc {
                                    x: to.x,
                                    y: to.y,
                                    i: routing::delta(centre.x, here.x),
                                    j: routing::delta(centre.y, here.y),
                                    ccw,
                                });
                            }
                        }
                        here = seg.end();
                    }
                    let last = path[path.len() - 1];
                    moves.push(RouteMove::Rapid { x: last.x, y: last.y, z: op.z.z_retract });
                    out.push_str(&render_moves(coder, render, moves, path[0], fs)?);
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
                    out.push_str(&render_moves(coder, render, moves, op.entry, fs)?);
                }
            }
        }
    }

    // One spindle stop closes the step (the next block/step's change_tool re-stops it,
    // harmlessly). Skipped when the step drilled nothing.
    if !step.blocks.is_empty() {
        let mut scope = Scope::new();
        out.push_str(&render_one(coder, "stop_spindle", &render.spindle_stop_tpl, &mut scope)?);
    }

    Ok(out)
}

/// Degrades a routing toolpath to what this machine can express, then renders it.
///
/// The single place a move list turns into text, so the fallback cannot be applied to one
/// op kind and forgotten on another. `from` is where the tool stands when the sequence
/// begins — [`degrade_moves`] needs it because an arc's start is implicit, being the
/// previous move's end.
fn render_moves(
    coder: &Coder,
    render: &StepRender,
    moves: Vec<RouteMove>,
    from: Point,
    fs: FeedsSpeeds,
) -> Result<String, BodyError> {
    let mut out = String::new();
    for mv in degrade_moves(moves, from, render) {
        out.push_str(&render_route_move(coder, render, &mv, fs)?);
    }
    Ok(out)
}

/// Rewrites moves this machine has no word for into ones it has.
///
/// The schema's `x-fallback` chain, applied: `cut_arc` → `cut_linear`. A blank `cut_arc`
/// does **not** mean "emit nothing" — it means the machine has no arc word, so say the
/// curve another way, to `curve_tolerance`. Every other blank primitive keeps its existing
/// meaning (a blank `tool_measure` still means *this machine needs no measurement block*),
/// which is exactly why the fallback is declared per-primitive in the schema rather than
/// inferred from emptiness here.
///
/// Done as a pass over the whole move list rather than inside [`render_route_move`]
/// because an arc's **start** is implicit — it is the previous move's end — and only a
/// walk of the sequence knows it. `from` is where the block begins.
///
/// `cut_linear` is the floor and has no fallback: a machine that cannot cut a straight
/// line has nothing left to fall back to, and the render fails rather than emitting a
/// program with the cuts missing.
fn degrade_moves(moves: Vec<RouteMove>, from: Point, render: &StepRender) -> Vec<RouteMove> {
    let has_arc = !render.cut_arc_tpl.trim().is_empty();
    if has_arc {
        return moves;
    }

    let tol = render.curve_tolerance;
    let mut out = Vec::with_capacity(moves.len());
    let mut here = from;
    // Z is carried from the last move that set it: a fitted arc replaces a lateral cut at
    // depth, and the chords standing in for it must be cut at that same depth.
    let mut depth = units::Length::from_mm(0.0);

    for mv in moves {
        match mv {
            RouteMove::Arc { x, y, i, j, ccw } => {
                let end = Point::new(x, y);
                let centre = Point::new(routing::sum(here.x, i), routing::sum(here.y, j));
                for point in arcfit::flatten_arc(here, end, centre, ccw, tol) {
                    out.push(RouteMove::Cut { x: point.x, y: point.y, z: depth });
                }
                here = end;
            }
            other => {
                match other {
                    RouteMove::Rapid { x, y, z }
                    | RouteMove::Plunge { x, y, z }
                    | RouteMove::Cut { x, y, z } => {
                        here = Point::new(x, y);
                        depth = z;
                    }
                    RouteMove::Arc { x, y, .. } => {
                        here = Point::new(x, y);
                    }
                }
                out.push(other);
            }
        }
    }
    out
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
        render_one(coder, "linear_cut", &render.cut_linear_tpl, &mut s)
    }

    match *mv {
        RouteMove::Rapid { x, y, z } => {
            let mut s = Scope::new();
            s.push("x", x);
            s.push("y", y);
            s.push("z", z);
            render_one(coder, "rapid_move", &render.move_rapid_tpl, &mut s)
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
/// The whole template for [`StepRender::opening_prompt`]: one script line calling the
/// machine's own `pause` primitive with the text.
///
/// A template rather than a direct call because `pause` is a **GTL callable** — it exists
/// on the engine, for profiles to invoke, and has no Rust-side entry point. Rendering a
/// one-line template is how Rust reaches it, and it keeps the prompt subject to whatever
/// the profile's `pause` actually emits (`M0`, `M00 (msg)`, nothing at all).
const OPENING_PROMPT_TPL: &str = "pause(prompt)\n";

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

/// Runs the CNC's `line_format` filter over every non-blank line of the assembled program.
///
/// A **filter**, not a generator: the template is handed `index` (0-based) and `text` (the
/// line as the generators built it) and emits the line that replaces it. It therefore owns
/// the whole line — the numbering word, the separator, and whether there is one at all:
///
/// ```text
/// line_format: "`N{(index + 1) * 10} {text}"     ->  N10 G0 X1 Y2
/// ```
///
/// This is a whole-program concern — no primitive can know its own position — so it runs
/// once here, over the finished program, rather than inside the templates that built it.
///
/// **Emitting nothing drops the line.** That is the mechanism by which a profile suppresses
/// one, and it is also the trap: a template that never emits `text` throws the G-code away
/// and leaves a column of bare line numbers. Nothing here can distinguish the two, so
/// nothing here tries — instead the variable an old prefix-style template used (`line`) no
/// longer exists, so such a template fails to render rather than quietly destroying the
/// program, and `normalize_cnc_value` warns about it at load.
///
/// An empty template returns the program unchanged. Blank lines are dropped, so the
/// filtered program is contiguous and `index` counts what the operator will actually see.
///
/// A template that fails to render is a [`BodyError::Render`]: shipping an unfiltered
/// program to a controller that requires line numbers would be worse than stopping.
pub fn format_lines(coder: &Coder, program: &str, template: &str) -> Result<String, BodyError> {
    if template.trim().is_empty() {
        return Ok(program.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for (index, line) in program.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let mut scope = Scope::new();
        scope.push("index", index as i64);
        scope.push("text", line.to_string());
        let formatted = render_one(coder, "line_format", template, &mut scope)?;
        // The filter owns the line, including its terminator: it emits one line's worth of
        // text (with the newline its own emit added), and the join below puts the newlines
        // back. A template that emitted nothing contributes no line at all.
        let formatted = formatted.trim_end_matches('\n');
        if !formatted.is_empty() {
            out.push(formatted.to_string());
        }
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
        tool_change_tpl: "`{manual_message}\n`T{slot} M06".to_string(),
        // Empty and off: a measurement block is machine-specific, and the tests that care
        // about it supply their own. Off by default keeps every other test's expected
        // output unchanged by its arrival.
        tool_measure_tpl: String::new(),
        measures_tool_length: false,
        // Front-face by default, so no prompt. The tests about the back-face prompt set it.
        opening_prompt: None,
        spindle_start_tpl: "`S{rpm}\n`M03".to_string(),
        spindle_stop_tpl: "`M05".to_string(),
        move_rapid_tpl: "`G0 X{x} Y{y} Z{z}".to_string(),
        cut_linear_tpl: "`G1 X{x} Y{y} Z{z} F{feedrate}".to_string(),
        cut_arc_tpl: r#"`{if clockwise { "G2" } else { "G3" }} X{x} Y{y} I{i} J{j} F{xy_feedrate}"#
            .to_string(),
        curve_tolerance: units::Length::from_mm(0.01),
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
    "`(k2g {filename} - {timestamp})\nmetric();\n`G0 Z{z_safe}".to_string()
}

/// See [`sample_initialise_tpl`].
#[cfg(test)]
pub(crate) fn sample_conclude_tpl() -> String {
    "`(end of file)".to_string()
}

/// See [`sample_initialise_tpl`]. The G-code pair lives here, in a *fixture*, precisely
/// because it may not live in the application: a real program's `G21` comes from the
/// profile's `set_unit` primitive.
#[cfg(test)]
pub(crate) fn sample_set_unit_tpl() -> String {
    "`{if metric { \"G21\" } else { \"G20\" }}".to_string()
}

/// See [`sample_initialise_tpl`]. A minimal `set_origin`: it validates nothing, because
/// what a machine accepts is that machine's business and the tests that care about
/// validation write their own template. This one only proves the value reaches the output.
#[cfg(test)]
pub(crate) fn sample_set_origin_tpl() -> String {
    "`{origin_reference}".to_string()
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

    /// A one-op step routing `path` as a contour span.
    fn contour_step(path: Vec<Point>) -> StepPlan {
        let (entry, exit) = (path[0], path[path.len() - 1]);
        StepPlan {
            index: 0,
            name: "Outline".to_string(),
            blocks: vec![ToolBlock {
                tool_id: "r1".to_string(),
                slot: Some(2),
                diameter: Length::from_mm(1.0),
                ops: vec![AtomicOp {
                    phase: Phase::Route,
                    kind: OpKind::RouteContour { path },
                    tool_id: "r1".to_string(),
                    entry,
                    exit,
                    z: ZProfile {
                        z_bottom: Length::from_mm(-2.1),
                        z_retract: Length::from_mm(5.0),
                        z_feed: None,
                    },
                    primitive: "route_contour",
                    source: "outline".to_string(),
                }],
                travel_mm: 0.0,
            }],
            notes: vec![],
        }
    }

    /// A 3.2 mm hole milled with a 1.0 mm router at (5,5) - its finishing lap is a full
    /// circle, which is the arc case worth testing.
    fn hole_step() -> StepPlan {
        StepPlan {
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
        }
    }

    /// A quarter circle of radius 10, sampled the way the stitcher tessellates one.
    fn quarter_circle() -> Vec<Point> {
        (0..=200)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (i as f64) / 200.0;
                Point::new(Length::from_mm(10.0 * a.cos()), Length::from_mm(10.0 * a.sin()))
            })
            .collect()
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

    /// A back-face program asks the operator to confirm the board was turned over,
    /// **before the first tool change** — while the spindle is parked and the board is
    /// still in their hands, not after a tool has been loaded to cut it.
    ///
    /// This prompt is load-bearing. Two symmetric pins of one diameter accept the board
    /// unflipped, and turned 180°, exactly as readily as the right way up; no geometry k2g
    /// has can tell those apart, so the question is the whole guard.
    #[test]
    fn a_back_face_program_opens_by_asking_whether_the_board_was_turned_over() {
        let coder = Coder::with_program_primitives(&crate::gcode::coder::ProgramPrimitives {
            set_unit: "",
            set_origin: "",
            origin_reference: "",
            comment: "`({text})",
            message: "`M117 {text}",
            pause: "`M00 ({text})",
        })
        .expect("primitives compile");

        let render = StepRender {
            opening_prompt: Some("Board back face up?".to_string()),
            ..render_ctx(true)
        };
        let body = render_step_body(
            &coder,
            &one_block_step(),
            &render,
            &feeds_for(Some(600.0), Some(12_000.0)),
        )
        .expect("body renders");

        let prompt = body
            .find("M00 (Board back face up?)")
            .unwrap_or_else(|| panic!("the prompt is emitted:\n{body}"));
        let first_change = body.find("M06").expect("the step still changes tools");
        assert!(prompt < first_change, "asked before anything is loaded:\n{body}");
    }

    /// A front-face step has nothing to confirm, so it emits nothing — the prompt must not
    /// become a line every program carries and every operator learns to click through.
    #[test]
    fn a_front_face_program_has_no_opening_prompt() {
        let coder = Coder::new();
        let body = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(true),
            &feeds_for(Some(600.0), Some(12_000.0)),
        )
        .expect("body renders");
        assert!(!body.contains("face up"), "no prompt on a front-face step:\n{body}");
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

    /// `line_format` emits the **whole line**: the number, its step, its separator and
    /// the G-code itself all come from the profile's template. The application supplies
    /// only `index` and `text`.
    #[test]
    fn the_line_filter_is_entirely_the_profiles_template() {
        let coder = Coder::new();
        // Blank lines are dropped; the rest step by whatever arithmetic the profile
        // writes, here the conventional ten from a 0-based index.
        assert_eq!(
            format_lines(&coder, "G21\n\nG0 Z5", "`N{(index + 1) * 10} {text}").unwrap(),
            "N10 G21\nN20 G0 Z5"
        );
        // A different dialect is a template edit, not a code change.
        assert_eq!(
            format_lines(&coder, "G21\nG0 Z5", "`/{index}:{text}").unwrap(),
            "/0:G21\n/1:G0 Z5"
        );
        // An empty template leaves the program exactly as the generators built it.
        assert_eq!(format_lines(&coder, "G21\nG0 Z5", "").unwrap(), "G21\nG0 Z5");
    }

    /// The filter owns the line, so a template that never emits `text` throws the G-code
    /// away. **The old prefix form is exactly that shape**, which is why `line` no longer
    /// exists: an un-migrated template fails to render rather than silently producing a
    /// column of bare line numbers. This is the guard for that.
    #[test]
    fn an_unmigrated_prefix_template_fails_rather_than_dropping_the_gcode() {
        let coder = Coder::new();
        let old_prefix_form = "`N{line * 10} `";
        let result = format_lines(&coder, "G21\nG0 Z5", old_prefix_form);
        assert!(
            result.is_err(),
            "the retired `line` variable must make this fail loudly, got: {result:?}"
        );
    }

    /// Emitting nothing for a line **drops** it — the mechanism by which a profile
    /// suppresses a line, and the reverse of the old behaviour where the application
    /// appended the line regardless.
    #[test]
    fn the_line_filter_can_rewrite_pass_through_or_drop_a_line() {
        let coder = Coder::new();

        // Pass a comment through unnumbered, number the rest. `index` still counts every
        // line, so the numbering reflects position in the program.
        let skip_comments = "if text.starts_with(\"(\") {\n    `{text}\n} else {\n    `N{(index + 1) * 10} {text}\n}";
        assert_eq!(
            format_lines(&coder, "(header)\nG21\n(done)\nG0 Z5", skip_comments).unwrap(),
            "(header)\nN20 G21\n(done)\nN40 G0 Z5"
        );

        // Emitting nothing removes the line altogether.
        let drop_comments = "if !text.starts_with(\"(\") {\n    `{text}\n}";
        assert_eq!(
            format_lines(&coder, "(header)\nG21\n(done)\nG0 Z5", drop_comments).unwrap(),
            "G21\nG0 Z5",
            "a line the filter does not emit is not in the program"
        );
    }

    /// The bundled templates carry real script now, not just substitution, and a Rhai
    /// error in one would otherwise surface as a failed generation in the field — a
    /// profile seeded from a template is never rendered until a job runs.
    #[test]
    fn every_bundled_line_filter_renders() {
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
                .pointer("/primitives/line_format")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("[{key}] has no line_format primitive"));

            let numbered = format_lines(&coder, "(a comment)\nG21", template)
                .unwrap_or_else(|e| panic!("[{key}] line_format failed to render: {e:?}"));
            let lines: Vec<&str> = numbered.lines().collect();
            assert_eq!(lines.len(), 2, "[{key}] must not drop a line");
            assert!(lines[1].contains("G21"), "[{key}] lost the line it numbers");
            // Only the Masso profiles opt out of numbering comments; the others are
            // pinned here as numbering everything, so a change to either is deliberate.
            if key.starts_with("masso") {
                assert_eq!(lines[0], "(a comment)", "[{key}] numbered a comment");
            } else {
                assert!(lines[0].starts_with('N'), "[{key}] left a line unnumbered");
                assert!(lines[0].contains("(a comment)"), "[{key}] dropped the comment text");
            }
        }
    }

    // ---- tool_measure -------------------------------------------------------------

    /// Emitted between `tool_change` and `spindle_start` — the tool has to be in the
    /// spindle and must not be cutting yet, which leaves exactly that gap.
    #[test]
    fn tool_measure_lands_between_the_change_and_the_spindle() {
        let coder = Coder::new();
        let render = StepRender {
            tool_measure_tpl: "`M998 T{slot} ({tool_name} {diameter})".to_string(),
            measures_tool_length: true,
            ..render_ctx(true)
        };
        let out =
            render_step_body(&coder, &one_block_step(), &render, &feeds_for(Some(300.0), Some(12_000.0)))
                .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let change = lines.iter().position(|l| l.contains("M06")).expect("tool change emitted");
        let measure = lines.iter().position(|l| l.contains("M998")).expect("measure emitted");
        let spindle = lines.iter().position(|l| l.starts_with('S')).expect("spindle started");
        assert!(change < measure && measure < spindle, "wrong order:\n{out}");
        assert!(lines[measure].contains("1.0mm drill"), "the tool is named: {}", lines[measure]);
    }

    /// A machine with an automatic setter measures at M06, so a block here would be a
    /// second, redundant cycle. The template is still present — the machine flag is what
    /// decides — so this cannot be mistaken for "the profile left it empty".
    #[test]
    fn tool_measure_is_skipped_on_a_machine_that_measures_itself() {
        let coder = Coder::new();
        let render = StepRender {
            tool_measure_tpl: "`M998".to_string(),
            measures_tool_length: false,
            ..render_ctx(true)
        };
        let out =
            render_step_body(&coder, &one_block_step(), &render, &feeds_for(Some(300.0), Some(12_000.0)))
                .unwrap();
        assert!(!out.contains("M998"), "an auto-setter machine measures at M06:\n{out}");
    }

    /// An empty template emits nothing even when the machine measures manually — the
    /// profile has simply not said what its measurement cycle is.
    #[test]
    fn an_empty_tool_measure_template_emits_nothing() {
        let coder = Coder::new();
        let render =
            StepRender { tool_measure_tpl: String::new(), measures_tool_length: true, ..render_ctx(true) };
        let with_measure =
            render_step_body(&coder, &one_block_step(), &render, &feeds_for(Some(300.0), Some(12_000.0)))
                .unwrap();
        let without = render_step_body(
            &coder,
            &one_block_step(),
            &render_ctx(true),
            &feeds_for(Some(300.0), Some(12_000.0)),
        )
        .unwrap();
        assert_eq!(with_measure, without, "no template, no output, not even a blank line");
    }

    // ---- drill modality -----------------------------------------------------------

    /// `index`/`count` let a profile open a modal cycle on the first hole, give bare
    /// coordinates for the rest, and cancel it on the last — which is the whole reason
    /// they exist, and what a G81 block is supposed to look like.
    #[test]
    fn a_modal_drill_template_opens_once_and_cancels_once() {
        let coder = Coder::new();
        let modal = "if index == 0 {\n    `G81 X{x} Y{y} Z{z_bottom} R{z_retract} F{z_feedrate}\n} else {\n    `X{x} Y{y}\n}\nif index == count - 1 {\n    `G80\n}";
        let mut step = one_block_step();
        step.blocks[0].ops.push(drill_op(20.0, 4.0)); // three holes
        let render = StepRender { drill_tpl: modal.to_string(), ..render_ctx(true) };

        let out = render_step_body(&coder, &step, &render, &feeds_for(Some(300.0), Some(12_000.0)))
            .unwrap();
        assert_eq!(out.matches("G81").count(), 1, "the cycle opens once:\n{out}");
        assert_eq!(out.matches("G80").count(), 1, "and is cancelled once:\n{out}");
        // Two holes after the first carry coordinates alone.
        assert_eq!(out.lines().filter(|l| l.starts_with("X")).count(), 2, "{out}");
        // Order: open, two bare moves, cancel.
        let lines: Vec<&str> = out.lines().collect();
        let open = lines.iter().position(|l| l.contains("G81")).unwrap();
        let cancel = lines.iter().position(|l| l.contains("G80")).unwrap();
        assert!(open < cancel, "the cycle must be cancelled after it is opened:\n{out}");
    }

    /// One hole opens *and* cancels the cycle — correct, not a special case the template
    /// has to guard.
    #[test]
    fn a_single_hole_block_opens_and_cancels_the_cycle() {
        let coder = Coder::new();
        let modal = "if index == 0 {\n    `G81 X{x} Y{y}\n}\nif index == count - 1 {\n    `G80\n}";
        let mut step = one_block_step();
        step.blocks[0].ops.truncate(1);
        let render = StepRender { drill_tpl: modal.to_string(), ..render_ctx(true) };

        let out = render_step_body(&coder, &step, &render, &feeds_for(Some(300.0), Some(12_000.0)))
            .unwrap();
        assert_eq!(out.matches("G81").count(), 1, "{out}");
        assert_eq!(out.matches("G80").count(), 1, "{out}");
    }

    /// `count` counts **drill ops only**. A block that also routes must not have its cycle
    /// "cancelled" early by a routing op being counted as a hole.
    #[test]
    fn the_drill_index_counts_holes_not_operations() {
        let coder = Coder::new();
        let mut step = one_block_step();
        // Two drills plus a routed hole in the same block.
        step.blocks[0].ops.push(AtomicOp {
            kind: OpKind::RouteHole { hole_diameter: Length::from_mm(3.0) },
            ..drill_op(30.0, 4.0)
        });
        let render =
            StepRender { drill_tpl: "`H{index}/{count}".to_string(), ..render_ctx(true) };

        let out = render_step_body(&coder, &step, &render, &feeds_for(Some(300.0), Some(12_000.0)))
            .unwrap();
        assert!(out.contains("H0/2") && out.contains("H1/2"), "two holes of two:\n{out}");
        assert!(!out.contains("/3"), "the routed hole is not a drill:\n{out}");
    }

    /// A template that cannot render stops generation: quietly shipping an unformatted
    /// program to a controller that requires line numbers is the worse failure.
    #[test]
    fn a_broken_line_filter_is_a_named_error() {
        let coder = Coder::new();
        let err = format_lines(&coder, "G21", "`N{nope}`").unwrap_err();
        match err {
            BodyError::Render { primitive, .. } => assert_eq!(primitive, "line_format"),
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

    /// A contour arriving as a fine polyline comes out as `G2`/`G3`.
    ///
    /// The whole point of the fitting pass. The routing offset is a polygon operation, so
    /// a curved board edge reaches here as hundreds of chords; emitting those verbatim is
    /// what produced the enormous programs this replaces.
    #[test]
    fn a_curved_contour_is_emitted_as_arcs_not_hundreds_of_chords() {
        let coder = Coder::new();
        let body =
            render_step_body(&coder, &contour_step(quarter_circle()), &render_ctx(true), &router_feed())
                .expect("routes");

        let arcs = body.lines().filter(|l| l.starts_with("G2 ") || l.starts_with("G3 ")).count();
        let cuts = body.lines().filter(|l| l.starts_with("G1 ")).count();
        assert_eq!(arcs, 1, "a quarter circle is one arc:\n{body}");
        assert_eq!(cuts, 1, "only the plunge stays a G1, got {cuts}:\n{body}");
    }

    /// A straight-sided contour must be untouched by the fitter. Every routed outline that
    /// exists today is one of these, so this is the regression guard on all of them.
    #[test]
    fn a_straight_contour_is_untouched_by_the_fitter() {
        let coder = Coder::new();
        let path: Vec<Point> = [(0.0, 0.0), (10.0, 0.0), (10.0, 8.0), (0.0, 8.0)]
            .iter()
            .map(|&(x, y)| Point::new(Length::from_mm(x), Length::from_mm(y)))
            .collect();
        let body = render_step_body(&coder, &contour_step(path), &render_ctx(true), &router_feed())
            .expect("routes");

        assert!(
            !body.contains("G2 ") && !body.contains("G3 "),
            "square corners must not become arcs:\n{body}"
        );
        for expect in ["G1 X10 Y0", "G1 X10 Y8", "G1 X0 Y8"] {
            assert!(body.contains(expect), "missing {expect}:\n{body}");
        }
    }

    /// A controller with no arc word cuts the arc as chords rather than losing it.
    ///
    /// Before the fallback chain an empty motion template rendered to nothing at all, so
    /// such a machine silently received a program with its routing missing — the failure
    /// this leg exists to end. Asserted on a routed hole, whose finishing lap is a full
    /// circle and so the worst case.
    #[test]
    fn an_empty_cut_arc_falls_back_to_straight_moves() {
        let coder = Coder::new();
        let render = StepRender { cut_arc_tpl: String::new(), ..render_ctx(true) };
        let body = render_step_body(&coder, &hole_step(), &render, &router_feed()).expect("routes");

        assert!(!body.contains("G2") && !body.contains("G3"), "no arc word is used:\n{body}");
        let cuts = body.lines().filter(|l| l.starts_with("G1 ")).count();
        assert!(cuts > 20, "the finishing lap must survive as chords, got {cuts}:\n{body}");

        // And it must still *be* a circle: chord vertices at the wall radius (1.1 mm) from
        // the hole centre at (5,5). A flattening that lost the geometry would still emit
        // G1s, so counting them alone proves nothing.
        let on_wall = body
            .lines()
            .filter_map(|l| {
                let x = l.split('X').nth(1)?.split_whitespace().next()?.parse::<f64>().ok()?;
                let y = l.split('Y').nth(1)?.split_whitespace().next()?.parse::<f64>().ok()?;
                Some((x, y))
            })
            .filter(|(x, y)| ((x - 5.0f64).hypot(y - 5.0) - 1.1).abs() < 0.02)
            .count();
        assert!(on_wall > 20, "the lap should trace the wall, found {on_wall} points:\n{body}");
    }

    /// The fitted arcs of a curved contour degrade too, so a machine with no arc word
    /// still gets the outline — as the chords it would have had before, not as nothing.
    #[test]
    fn a_curved_contour_still_cuts_on_a_machine_with_no_arc_word() {
        let coder = Coder::new();
        let render = StepRender { cut_arc_tpl: String::new(), ..render_ctx(true) };
        let body =
            render_step_body(&coder, &contour_step(quarter_circle()), &render, &router_feed())
                .expect("routes");

        assert!(!body.contains("G2") && !body.contains("G3"), "{body}");
        let cuts = body.lines().filter(|l| l.starts_with("G1 ")).count();
        assert!(cuts > 10, "the curve must still be cut, got {cuts} moves:\n{body}");
    }

    /// A tighter tolerance buys more chords — proof the profile field reaches the geometry
    /// rather than a constant being used behind it.
    #[test]
    fn curve_tolerance_governs_the_flattening() {
        let coder = Coder::new();
        let count = |tol: f64| {
            let render = StepRender {
                cut_arc_tpl: String::new(),
                curve_tolerance: Length::from_mm(tol),
                ..render_ctx(true)
            };
            render_step_body(&coder, &hole_step(), &render, &router_feed())
                .expect("routes")
                .lines()
                .filter(|l| l.starts_with("G1 "))
                .count()
        };
        let (fine, coarse) = (count(0.001), count(0.05));
        assert!(fine > coarse, "tighter tolerance must give more chords: {fine} vs {coarse}");
    }
}
