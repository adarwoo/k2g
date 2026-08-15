//! Operation-planner adapter — builds the in-memory [`MachiningPlan`] the Job
//! "Machining" view renders (operation-planner.md). It resolves each machining step
//! (operations, drill config, toolset, CNC), runs the **same** tool assignment as the
//! Tooling tab (so the machining blocks and the rack agree), then hands the resolved
//! drill targets to the pure [`planner`](crate::gcode::planner) for decomposition and
//! ordering.
//!
//! **Scope.** Both phases are planned. Round PTH/NPTH holes (and vias) become ordered
//! point-drill ops or spiral-routed pockets; oblong slots become drill chains, router
//! passes or both, per the step's strategy; the board outline becomes offset cut spans
//! with retaining tabs left between them; and a step may machine either face of the board.
//!
//! Two things are deliberately still notes rather than ops: **scoring / V-grooving**
//! (partial-depth cuts need a depth model and a V-bit the tool stock does not describe)
//! and **arc-preserving outline offsets** — the outline is offset as a polyline today, so
//! a curved edge is cut as chords rather than as `G2`/`G3` (op-planner §3, §9.6). Neither
//! produces a wrong program; each produces a less complete or less elegant one, and says
//! so.
//!
//! ## One frame for the whole job
//!
//! Locating pins are the one operation whose geometry comes from the **fixture** rather
//! than from the board, and they are what makes double-sided work possible: two holes on
//! the fixture's flip line, drilled through the board and into the backboard, so it can be
//! turned over and land back where it was.
//!
//! They also grow the job's coordinate frame, because they sit outside the board — and
//! that growth is decided **once, here**, from the profile's locating-pins step, and given
//! to every step and to the 3D workpiece ([`JobFrame`]). Never per step: the operator
//! drills one set of pins and every program of the job has to be written against the same
//! zero, or the second setup cuts somewhere the first did not.

use std::sync::Arc;

use uuid::Uuid;

use units::{Length, UserUnitDisplay};

use crate::data::model::{FixtureProfile, TabContour, Tool};
use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{self, AssignConfig, AssignError, Strategy, Weights};
use crate::gcode::placement::{BoardFlip, BoardOrigin, Margin, Placement, PlacementSpec};
use crate::gcode::plan::{MachiningPlan, Point, StepPlan};
use crate::gcode::planner::{
    plan_drilling, plan_engrave, plan_outline, plan_routing, DrillTarget, EngraveSpan, OutlineSpan,
    RouteShape, RouteTarget,
};
use crate::gcode::{oblong, outline, pins, scene};
use crate::runtime::isolation::IsolationSpec;
use crate::runtime::tooling::{
    build_rack_spec, build_setup, collect_hole_groups, missing_bindings, pick_engraver,
    pick_pin_tool, plan_routers, read_steps, HoleGroup, PinTool, RouterPlan, StepRaw,
};
use crate::runtime::AppCtx;

/// The coordinate frame every program of one job is written in.
///
/// Derived once per job (see the module note) so the steps cannot disagree about where the
/// zero is. Without locating pins it is entirely inert — a default [`Margin`] and no pin
/// diameter — and the transform is bit-for-bit the one k2g produced before any of this
/// existed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JobFrame {
    /// Room the origin makes for the pins. Zero when the job has none.
    pub margin: Margin,
    /// The pin diameter, when the job drills pins at all.
    pub pin_diameter: Option<Length>,
    /// Which axis the board turns about, from the fixture. Meaningful for the pins even on
    /// an all-front job, because it decides which pair of sides they sit on.
    pub flip_axis: BoardFlip,
}

/// The job's shared frame, from the profile's steps and the fixture holding the board.
///
/// The pin diameter is taken from **the first step that drills pins**, which the readiness
/// gate has already established is step 1 if it exists at all
/// ([`locating_pin_faults`](crate::runtime::tooling::locating_pin_faults)). Taking the
/// first rather than, say, the largest is the honest reading of "the pins this job is
/// registered by": a second pin step re-fixtures against the same holes.
///
/// The flip axis comes from the fixture rather than from the step because it is a fact
/// about where the registration *is*, which the fixture owns.
fn job_frame(steps: &[StepRaw], fixture: Option<&FixtureProfile>) -> JobFrame {
    let flip_axis = fixture
        .map(|f| BoardFlip::from_axis(&f.board_flip_axis))
        .unwrap_or(BoardFlip::AboutY);
    let pin_diameter = steps
        .iter()
        .find(|step| step.drills_locating_pins())
        .and_then(|step| step.pin_diameter);

    JobFrame {
        margin: pin_diameter
            .map(|d| pins::margin(d, flip_axis))
            .unwrap_or_default(),
        pin_diameter,
        flip_axis,
    }
}

/// The fixture a job's frame is measured in: the one the **first** step is set up in.
///
/// A profile whose steps name different fixtures is a profile whose steps cannot share a
/// zero, which is a different problem from this one; taking the first keeps the frame a
/// single value rather than silently picking whichever fixture happened to be looked up
/// last.
fn frame_fixture<'a>(ctx: &'a AppCtx, steps: &[StepRaw]) -> Option<&'a FixtureProfile> {
    let id = steps.first()?.fixture_id?;
    ctx.fixtures.iter().find(|f| f.id == id.to_string())
}

/// What a cached plan was built from.
///
/// Two counters, and no list of the fields the planner happens to read. A key assembled
/// field by field is a key someone has to remember to extend, and the failure when they
/// do not is the worst kind this program has: a plan that looks current, prices a job
/// correctly, and describes work the operator has already changed.
///
/// The two stores are counted separately because they change independently — a step's
/// operations are edited straight into the datastore without the context hearing about
/// it, and the board arrives in the context without the datastore hearing about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlanKey {
    context: u64,
    data: u64,
}

impl PlanKey {
    fn of(ctx: &AppCtx) -> Self {
        PlanKey { context: ctx.revision, data: crate::data::data_revision() }
    }
}

/// The last machining plan and the key it was built under.
///
/// Cloning shares rather than copies, so every snapshot of the context looks at the same
/// cell — which is the point, since the UI takes snapshots continuously and the job
/// changes rarely.
type CachedPlan = std::sync::Mutex<Option<(PlanKey, Arc<MachiningPlan>)>>;

#[derive(Clone, Default)]
pub struct PlanCache(std::sync::Arc<CachedPlan>);

impl std::fmt::Debug for PlanCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlanCache")
    }
}

impl PlanCache {
    /// The plan for `key`, calling `build` only if the one held is for something else.
    ///
    /// The lock is dropped before `build` runs. Two threads arriving together will both
    /// build and the second will overwrite the first with an identical answer — one
    /// wasted run, in exchange for never holding a mutex across the heaviest call in the
    /// program while a UI thread waits behind it.
    fn get_or_build(
        &self,
        key: PlanKey,
        build: impl FnOnce() -> MachiningPlan,
    ) -> Arc<MachiningPlan> {
        if let Ok(cache) = self.0.lock() {
            if let Some((cached, plan)) = cache.as_ref() {
                if *cached == key {
                    return Arc::clone(plan);
                }
            }
        }
        let plan = Arc::new(build());
        if let Ok(mut cache) = self.0.lock() {
            *cache = Some((key, Arc::clone(&plan)));
        }
        plan
    }
}

/// The machining plan for this context, planning it only if nothing else already has.
///
/// **Use this, not [`plan_machining`].** The Machining view and the 3D view render from
/// the same plan and re-render for reasons that have nothing to do with the job — a tool
/// hidden in the legend, a section collapsed, the dock dragged — and planning a dense
/// board takes long enough to be felt. Nothing about a render changes what the plan is.
///
pub fn cached_plan(ctx: &AppCtx) -> Arc<MachiningPlan> {
    ctx.plan_cache.get_or_build(PlanKey::of(ctx), || plan_machining(ctx))
}

/// Builds the machining plan for the current context: one [`StepPlan`] per machining
/// step of the selected profile, each with its ordered drill-phase tool blocks.
///
/// Plans unconditionally. Callers on a render path want [`cached_plan`] instead.
pub fn plan_machining(ctx: &AppCtx) -> MachiningPlan {
    let Some(profile_id) = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return note("Select a machining profile to plan machining.");
    };
    if ctx.board.is_none() {
        return note("No board loaded — nothing to machine.");
    }
    if !appdata_ready() {
        return note("Configuration store is not ready.");
    }

    let raw_steps = read_steps(profile_id);
    if raw_steps.is_empty() {
        return note("The machining profile has no steps.");
    }

    // The job's board orientation is applied by the Placement (board → machine).
    let orientation = with_appdata(|data| data.job_board_orientation()) as f64;
    // Derived from the whole profile, before any step is planned, and identical for all of
    // them — see the module note.
    let frame = job_frame(&raw_steps, frame_fixture(ctx, &raw_steps));

    let steps = raw_steps
        .iter()
        .enumerate()
        .map(|(index, raw)| plan_step(ctx, index, raw, orientation, &frame))
        .collect();

    MachiningPlan { steps, note: None }
}

/// The workpiece as the 3D view draws it: the stitched outline, its interior cutouts and
/// every drilled hole, all placed into machine space so the board and the toolpaths share
/// one frame.
///
/// Placed by **`step`'s own** fixture origin and CNC scaling, because that is the setup
/// its toolpaths are drawn in. This used to take the first step's placement and call it an
/// approximation; once the view shows one step at a time that justification is gone — a
/// second step in a different fixture would have had its paths drawn against a workpiece
/// positioned by the first step's origin.
///
/// `None` when there is no board or the outline could not be stitched — the toolpaths
/// still render, just without a workpiece under them.
pub fn board_solid(ctx: &AppCtx, step: usize) -> Option<scene::BoardSolid> {
    let board = ctx.board.as_ref()?;
    let stitched = ctx.stitched_board_data.as_ref()?;
    if !stitched.errors.is_empty() {
        return None;
    }

    let orientation = with_appdata(|data| data.job_board_orientation()) as f64;
    let all_steps: Vec<StepRaw> = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(read_steps)
        .unwrap_or_default();
    let raw = all_steps.get(step);
    let cnc = raw
        .and_then(|raw| raw.cnc_id)
        .and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()));
    let fixture = raw
        .and_then(|raw| raw.fixture_id)
        .and_then(|id| ctx.fixtures.iter().find(|f| f.id == id.to_string()));

    // The same frame the toolpaths are planned in — derived from the whole profile, not
    // from this step — so the workpiece and the paths drawn over it cannot disagree about
    // where the zero is.
    let frame = job_frame(&all_steps, frame_fixture(ctx, &all_steps));

    // Z here is irrelevant — a solid is placed in XY only — so the retract/safe heights
    // are nominal rather than resolved from a fixture.
    let placement = Placement::new(&PlacementSpec {
        bounds: board.bounding_box.as_ref(),
        orientation_deg: orientation,
        origin: fixture
            .map(|f| BoardOrigin::from_edges(&f.origin_x0, &f.origin_y0))
            .unwrap_or_default(),
        margin: frame.margin,
        // Turned over exactly when this step machines the bottom, which is what mirrors
        // the artwork so it is drawn as the operator will physically see it.
        flip: raw
            .is_some_and(|raw| raw.machines_back)
            .then_some(frame.flip_axis),
        scale_x: cnc.map(|m| m.scaling_x as f64).unwrap_or(1.0),
        scale_y: cnc.map(|m| m.scaling_y as f64).unwrap_or(1.0),
        z_retract: Length::from_mm(0.0),
        z_safe: Length::from_mm(0.0),
    });
    let place = |&(x, y): &(i64, i64)| {
        let point = placement.xy(&pcb::BoardPoint {
            x: Length::from_mm(x as f64 / 1e6),
            y: Length::from_mm(y as f64 / 1e6),
        });
        [point.x.as_mm(), point.y.as_mm()]
    };

    let mut solid = scene::BoardSolid {
        // A board with no stitched outer boundary has nothing to extrude.
        outline: stitched
            .contours
            .iter()
            .find(|c| !c.is_hole)
            .map(|c| c.points.iter().map(place).collect())?,
        openings: stitched
            .contours
            .iter()
            .filter(|c| c.is_hole)
            .map(|c| c.points.iter().map(place).collect())
            .collect(),
        thickness_mm: board
            .thickness
            .map(|t| t.as_mm())
            .unwrap_or(DEFAULT_THICKNESS_MM),
        // Which way up the board is lying, so the renderer knows which of its two faces
        // the spindle is looking at. The back is drawn red and the front green either way;
        // this only says which one is on top.
        back_face_up: raw.is_some_and(|raw| raw.machines_back),
    };

    // Drilled holes, at their finished size — the board as it will come off the machine,
    // not the tool list that got it there.
    for hole in &board.holes {
        let placed = placement.xy(&hole.position);
        let diameter = hole
            .drill_axes()
            .map(|(major, _)| major.as_mm())
            .unwrap_or_default();
        solid.add_hole(placed.x.as_mm(), placed.y.as_mm(), diameter);
    }

    Some(solid)
}

/// The setup around the workpiece: where the work zero is, which way the fixture's stop
/// faces, and where the locating pins go.
///
/// Drawn from the same [`JobFrame`] and the same [`Placement`] as the board and the
/// toolpaths, so "the board floats away from the bracket" is a true statement about the
/// program rather than a drawing convention. With pins that gap is exactly the margin the
/// origin made for them — which is the one thing about this frame an operator cannot
/// otherwise see.
///
/// `None` when there is no board, because every part of it is measured from one.
pub fn fixture_scene(ctx: &AppCtx, step: usize) -> Option<scene::FixtureMark> {
    let board = ctx.board.as_ref()?;
    let bounds = board.bounding_box.as_ref()?;

    let orientation = with_appdata(|data| data.job_board_orientation()) as f64;
    let all_steps: Vec<StepRaw> = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(read_steps)
        .unwrap_or_default();
    let raw = all_steps.get(step);
    let fixture = frame_fixture(ctx, &all_steps);
    let frame = job_frame(&all_steps, fixture);
    let origin = fixture
        .map(|f| BoardOrigin::from_edges(&f.origin_x0, &f.origin_y0))
        .unwrap_or_default();

    let placement = Placement::new(&PlacementSpec {
        bounds: Some(bounds),
        orientation_deg: orientation,
        origin,
        margin: frame.margin,
        flip: raw
            .is_some_and(|raw| raw.machines_back)
            .then_some(frame.flip_axis),
        scale_x: 1.0,
        scale_y: 1.0,
        z_retract: Length::from_mm(0.0),
        z_safe: Length::from_mm(0.0),
    });

    let rect = placement.board_rect_mm();
    // Long enough to read as a stop rather than a tick, short enough not to dominate a
    // small board: a quarter of the board's larger side.
    let arm_mm = ((rect.max_x - rect.min_x).max(rect.max_y - rect.min_y) / 4.0).max(5.0);

    Some(scene::FixtureMark {
        arm_mm,
        // The arms run along the work, i.e. away from the stop the board is pushed into.
        // With `x0: right` the board is at negative X, so the arm goes that way too.
        dir_x: if origin.x_at_right { -1.0 } else { 1.0 },
        dir_y: if origin.y_at_far { -1.0 } else { 1.0 },
        pins: frame
            .pin_diameter
            .map(|diameter| {
                pins::centres(rect, frame.flip_axis, diameter)
                    .iter()
                    .map(|p| [p.x.as_mm(), p.y.as_mm(), diameter.as_mm()])
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Board thickness assumed when the KiCad stackup does not report one. 1.6 mm is the
/// overwhelmingly common PCB, and this only affects how the workpiece is *drawn*.
const DEFAULT_THICKNESS_MM: f64 = 1.6;

/// A whole-plan note (nothing to plan).
fn note(message: &str) -> MachiningPlan {
    MachiningPlan {
        steps: vec![],
        note: Some(message.to_string()),
    }
}

/// Plans one step's drill phase, in the job's shared coordinate `frame`.
fn plan_step(
    ctx: &AppCtx,
    index: usize,
    raw: &StepRaw,
    orientation: f64,
    frame: &JobFrame,
) -> StepPlan {
    let name = raw.name.clone();
    let mut notes: Vec<String> = Vec::new();
    let has_pth = raw.drills_pth();
    let has_npth = raw.drills_npth();
    let has_route = raw.routes_outline();
    let has_locating = raw.drills_locating_pins();

    // Every binding is required to plan. Defaulting a missing CNC to "no ATC, unity
    // scaling" or a missing fixture to nominal heights would produce a plausible-looking
    // program for hardware the operator does not have, so an unset binding stops the
    // step. Shared with the Tooling tab so both views refuse the same steps.
    if let Some(reason) = missing_bindings(raw) {
        return failed(index, name, vec![reason]);
    }

    // A back-face step used to be refused here, because nothing mirrored its geometry and
    // the plan it produced would silently have described the top side. The mirror now lives
    // in the `Placement` built below, applied from the fixture's own flip axis, so a
    // back-face step plans like any other.
    let (Some(cnc_id), Some(fixture_id), Some(toolset_id)) =
        (raw.cnc_id, raw.fixture_id, raw.toolset_id)
    else {
        unreachable!("missing_bindings just established all three are present")
    };
    let Some(toolset) = ctx.toolsets.iter().find(|t| t.id == toolset_id.to_string()) else {
        return failed(
            index,
            name,
            vec!["The step's toolset profile could not be found.".into()],
        );
    };
    let Some(cnc) = ctx.machines.iter().find(|m| m.id == cnc_id.to_string()) else {
        return failed(
            index,
            name,
            vec!["The step's CNC profile could not be found.".into()],
        );
    };
    let Some(fixture) = ctx.fixtures.iter().find(|f| f.id == fixture_id.to_string()) else {
        return failed(
            index,
            name,
            vec!["The step's fixture profile could not be found.".into()],
        );
    };
    let atc_slots = cnc.atc_slot_count as usize;

    // A fixture set up in an origin this controller does not have was once caught here, by
    // comparing the fixture's *ordinal* against a count on the CNC profile. Both are gone:
    // the fixture now names its origin the way the machine does, and the machine's
    // `set_origin` primitive is the single authority on which names it accepts — a count
    // could never have expressed a MASSO's `G54.1 P1..P100`. The check happens when the
    // program is generated, and refuses it outright rather than warning.

    let holes: &[pcb::BoardHole] = ctx
        .board
        .as_ref()
        .map(|b| b.holes.as_slice())
        .unwrap_or(&[]);
    let groups = collect_hole_groups(holes, has_pth, has_npth);

    // The rack must reserve every router routing requires — the outline cutter and one
    // per slot width, since a cutter wider than a slot cannot mill it. Resolved by the
    // shared planner so this and the Tooling tab produce the same rack, and thus the
    // same slot numbers.
    let has_oblongs = groups.iter().any(|g| g.minor.is_some());
    let oblong = raw.oblong_strategy();
    let cutout_contours =
        crate::runtime::tooling::cutout_contours(ctx.stitched_board_data.as_ref(), raw);
    let routers = plan_routers(
        &ctx.tools,
        toolset,
        &groups,
        has_route,
        raw.route_board.kerf,
        has_oblongs && oblong.routes(),
        crate::runtime::tooling::CutoutRouting {
            contours: &cutout_contours,
            relieve_corners: raw.routes_cutouts() && raw.route_cutouts.drill_sharp_corners,
        },
    );

    // The pin hole's tool, resolved before the assigner runs and deliberately outside it —
    // see `pick_pin_tool`. Refused rather than substituted: a registration hole that is
    // nearly the right size does not register.
    //
    // The diameter is the **job frame's**, not this step's own. The two can only ever be
    // the same — a profile with a second locating-pins step is a readiness fault, since
    // pins must be the first step — and taking the frame's is what *guarantees* it: the
    // pin centres below are measured out with this diameter, and the origin made room for
    // exactly that. Reading the step's would let the two drift and put a pin hole outside
    // the frame that was opened for it.
    let pin_diameter = has_locating.then_some(frame.pin_diameter).flatten();
    if has_locating && pin_diameter.is_none() {
        // The schema materialises a diameter on every step, so this is a hand-edited or
        // truncated profile. Refused rather than quietly planning a step that drills no
        // pins: the steps after it are about to be cut against registration that does not
        // exist.
        return failed(
            index,
            name,
            vec![
                "This step drills locating pins but no pin diameter is set. Choose one in \
                  the machining profile."
                    .into(),
            ],
        );
    }
    let pin_tool = match pin_diameter {
        Some(diameter) => match pick_pin_tool(&ctx.tools, toolset, diameter) {
            Some(tool) => Some(tool),
            None => {
                return failed(
                    index,
                    name,
                    vec![format!(
                        "No tool can make the {} locating-pin holes: there is no drill of \
                         exactly that diameter in stock, and no router narrow enough to mill \
                         one. A registration hole is never drilled to a nearly-right size, so \
                         this step cannot be planned — stock a {} drill.",
                        fmt_len(ctx, diameter),
                        fmt_len(ctx, diameter),
                    )],
                );
            }
        },
        None => None,
    };

    // The V-bit, chosen the way the pin tool is: outside the assigner, which scores holes
    // and has nothing to say about a channel width. `None` here means no bit in stock can
    // cut the width asked for, which is a note rather than a failure — the step's other
    // work is still worth doing.
    // Fails the step rather than noting it, and the Tooling adapter fails the same way
    // with the same wording — which is what puts it in the diagnostics banner and shuts
    // the generation gate. A program that drilled and routed and quietly did not engrave
    // would look finished to anyone who ran it.
    let engraver = if raw.engraves_copper() {
        match pick_engraver(&ctx.tools, toolset, raw.engrave_copper.width) {
            Some(picked) => Some(picked),
            None => {
                return failed(
                    index,
                    name,
                    vec![crate::runtime::tooling::no_engraver_reason(
                        ctx,
                        raw.engrave_copper.width,
                    )],
                )
            }
        }
    } else {
        None
    };

    // Nothing to assign *and* nothing to route. Cutouts count as work in their own right:
    // a step that only cuts interior openings has no holes, no outline and no pins, and
    // before they were an operation of their own that combination could only mean an empty
    // step. Engraving is the same case again — the copper is work no hole or contour
    // accounts for.
    if groups.is_empty()
        && !has_route
        && !raw.routes_cutouts()
        && engraver.is_none()
        && pin_tool.is_none()
    {
        return StepPlan {
            index,
            name,
            blocks: vec![],
            notes,
        };
    }

    // Assemble assigner inputs identically to the tooling adapter.
    let demands: Vec<_> = groups.iter().map(HoleGroup::to_demand).collect();
    let cfg = AssignConfig {
        allow_routing_holes: raw.drill.route_fallback,
        drill_first: raw.drill.drill_first,
        pilot: raw.drill.pilot,
        oversize: raw.drill.oversize,
        undersize: raw.drill.undersize,
        weights: Weights::default(),
    };
    // Shared with the Tooling tab so the two views agree on tool feasibility.
    let setup = build_setup(ctx, raw.fixture_id);
    // The pin tool joins the routers as mandatory: it is chosen outside the assigner, so
    // nothing else would reserve it a slot, and a step that cannot load it cannot register
    // the board.
    let mut mandatory = routers.mandatory_ids();
    if let Some(tool) = pin_tool.as_ref() {
        mandatory.push(tool.id().to_string());
    }
    // The engraver too, and for the same reason: a step that only engraves demands no
    // holes at all, so nothing else would reserve it a slot and the one bit the step
    // exists to use would be the one bit the rack does not hold.
    if let Some((tool_id, _)) = engraver.as_ref() {
        mandatory.push(tool_id.clone());
    }
    mandatory.sort();
    mandatory.dedup();
    let rack = build_rack_spec(toolset, atc_slots, &mandatory);

    let assignment = match assigner::assign(&demands, &ctx.tools, &cfg, &rack, &setup) {
        Ok(assignment) => assignment,
        Err(error) => return failed(index, name, format_assign_error(&error)),
    };

    // Tool id → rack slot, for the block's display.
    let slots: std::collections::BTreeMap<String, u8> = assignment
        .rack
        .iter()
        .map(|s| (s.tool_id.clone(), s.slot))
        .collect();

    // Turn each round hole into a target: a point-drill when a drill was assigned, or a
    // spiral route when the assigner fell back to a router (too big to drill, or a drill
    // point that would reach the bed). Oblong slots are still deferred.
    let mut drill_targets: Vec<DrillTarget> = Vec::new();
    let mut route_targets: Vec<RouteTarget> = Vec::new();
    // Slots whose strategy calls for a router but for which no cutter fits, and slot
    // routers whose flute is too short to reach through — both leave the slot unfinished.
    let mut unmilled_slots = 0usize;
    let mut short_flute_routers: std::collections::BTreeSet<String> = Default::default();
    for (i, hole) in holes.iter().enumerate() {
        let Some(group) = HoleGroup::from_hole(hole, has_pth, has_npth) else {
            continue;
        };
        let Some(assigned) = assignment.holes.iter().find(|h| h.hole_id == group.id()) else {
            continue;
        };
        let Some(tool_diameter) = ctx
            .tools
            .iter()
            .find(|t| t.id == assigned.tool_id)
            .map(|t| t.diameter)
        else {
            continue;
        };
        let source = hole.id.clone().unwrap_or_else(|| format!("hole#{i}"));

        // An oblong made by drilling: the assigner sized the drill to the slot's minor
        // axis, so that same drill walks the major axis. The slot's *route* half (the
        // web, or the wall cleanup) still belongs to the route phase.
        if let Some(slot) = hole.slot() {
            if oblong.drills() && assigned.strategy == Strategy::Drill {
                let positions =
                    oblong::chain_positions(&slot, tool_diameter, oblong.chain_pitch_fraction());
                for (n, at) in positions.into_iter().enumerate() {
                    drill_targets.push(DrillTarget {
                        source: format!("{source}.{n}"),
                        at,
                        tool_id: assigned.tool_id.clone(),
                        diameter: tool_diameter,
                        z_bottom: assigned.z_bottom,
                        // One run: the chain order is already chosen, so the TSP must
                        // place the chain without resequencing inside it.
                        chain: Some(source.clone()),
                    });
                }
            }
            // The slot's route half. The cutter is chosen by *width* (it must fit
            // between the walls), so it is the router plan's, not the assigner's — and
            // its plunge has no drill point to clear.
            if oblong.routes() {
                let router = routers
                    .for_group(&group)
                    .and_then(|id| ctx.tools.iter().find(|t| t.id == id));
                match router {
                    Some(router) => {
                        let z_bottom = assigner::router_plunge(&setup);
                        if router
                            .flute_length
                            .is_some_and(|f| f.as_mm() < z_bottom.as_mm())
                        {
                            short_flute_routers.insert(router.name.clone());
                        }
                        // The medial axis: the two end centres, which is exactly where a
                        // drill making the ends would sit.
                        let half = Length::from_mm(slot.travel().as_mm() / 2.0);
                        route_targets.push(RouteTarget {
                            source: format!("{source}.route"),
                            at: slot.point_at(Length::from_mm(-half.as_mm())),
                            tool_id: router.id.clone(),
                            tool_diameter: router.diameter,
                            shape: RouteShape::Slot {
                                far: slot.point_at(half),
                                width: slot.width,
                                from_solid: oblong.routes_from_solid(),
                            },
                            z_bottom,
                        });
                    }
                    None => unmilled_slots += 1,
                }
            }
            continue;
        }

        if assigned.strategy == Strategy::Drill {
            drill_targets.push(DrillTarget {
                source,
                at: hole.position.clone(),
                tool_id: assigned.tool_id.clone(),
                diameter: tool_diameter,
                z_bottom: assigned.z_bottom,
                chain: None,
            });
        } else {
            route_targets.push(RouteTarget {
                source,
                at: hole.position.clone(),
                tool_id: assigned.tool_id.clone(),
                tool_diameter,
                shape: RouteShape::Hole {
                    hole_diameter: group.target,
                },
                z_bottom: assigned.z_bottom,
            });
        }
    }

    // Place ops in machine space and order each phase: drilling first (board rigid),
    // then the route-hole phase (op-planner §4).
    let placement = Placement::new(&PlacementSpec {
        bounds: ctx.board.as_ref().and_then(|b| b.bounding_box.as_ref()),
        orientation_deg: orientation,
        origin: BoardOrigin::from_edges(&fixture.origin_x0, &fixture.origin_y0),
        // The job's margin, not this step's: every program is written against the one zero
        // the operator set up against.
        margin: frame.margin,
        // The board is physically turned over for a back-face step, so its geometry
        // mirrors about the line the pins sit on. Everything downstream places through
        // this, so nothing else has to know which side is being cut.
        flip: raw.machines_back.then_some(frame.flip_axis),
        scale_x: cnc.scaling_x as f64,
        scale_y: cnc.scaling_y as f64,
        z_retract: fixture.z_retract,
        z_safe: fixture.z_safe,
    });
    let start = Point::new(Length::from_mm(0.0), Length::from_mm(0.0));

    // The locating pins. Measured from the *placed* board and then unplaced, the way the
    // outline's mouse-bite centres are, because they are fixture geometry rather than
    // board geometry — KiCad has nothing to say about where they go.
    if let (Some(tool), Some(diameter)) = (pin_tool.as_ref(), pin_diameter) {
        let z_bottom = pin_plunge(&setup);
        if let Some(shortfall) = shallow_pin_engagement(&setup) {
            notes.push(format!(
                "Locating pins engage only {} into the backboard, which is not enough to \
                 hold the board square — use a thicker backboard or reduce the bed \
                 clearance.",
                fmt_len(ctx, shortfall),
            ));
        }
        for (n, centre) in pins::centres(placement.board_rect_mm(), frame.flip_axis, diameter)
            .into_iter()
            .enumerate()
        {
            let at = placement.unplace(&centre);
            match tool {
                PinTool::Drill { id, diameter: bit } => drill_targets.push(DrillTarget {
                    source: format!("pin.{n}"),
                    at,
                    tool_id: id.clone(),
                    diameter: *bit,
                    z_bottom,
                    // One run, so the two pins are drilled one after the other rather than
                    // being scattered through the tour with the board's own holes between
                    // them. They are the datum: they want making together.
                    chain: Some("pin".to_string()),
                }),
                PinTool::Router {
                    id,
                    diameter: cutter,
                } => route_targets.push(RouteTarget {
                    source: format!("pin.{n}"),
                    at,
                    tool_id: id.clone(),
                    tool_diameter: *cutter,
                    shape: RouteShape::Hole {
                        hole_diameter: diameter,
                    },
                    z_bottom,
                }),
            }
        }
    }

    // The board outline. Planned before the blocks are built because its mouse bites are
    // *drilled*, and they have to join the drill phase — the board must still be whole
    // when they are made, or the perforation is cut into a board that is already loose.
    let mut outline_spans: Vec<OutlineSpan> = Vec::new();
    if has_route {
        if raw.route_board.cuts_through() {
            match plan_outline_spans(ctx, raw, &routers, &placement, &mut drill_targets, &slots) {
                Ok((spans, warnings)) => {
                    notes.extend(warnings);
                    if spans.is_empty() {
                        notes.push(
                            "The retaining tabs are wider than the outline they sit on, so \
                             nothing would be cut. Reduce the tab width or the tab count."
                                .into(),
                        );
                    }
                    outline_spans = spans;
                }
                Err(reason) => notes.push(reason),
            }
        } else {
            notes.push(format!(
                "Edge cut '{}' is not yet planned — only 'route' and 'mill' cut right \
                 through. Scoring and V-grooving need a partial-depth model and a V-bit \
                 the tool stock does not carry yet.",
                raw.route_board.cut
            ));
        }
    }

    // The interior openings, when the step claims them in their own right. Planned
    // before the blocks for the same reason the outline is: its corner relief and mouse
    // bites are drilled, and they belong to the drill phase.
    let mut cutout_spans: std::collections::BTreeMap<String, Vec<OutlineSpan>> = Default::default();
    if raw.routes_cutouts() {
        let (spans, warnings) = plan_cutout_spans(
            ctx,
            raw,
            &routers,
            &placement,
            &mut drill_targets,
            &slots,
            &setup,
        );
        notes.extend(warnings);
        cutout_spans = spans;
    }

    // The isolation pass, first of all the blocks. `Phase` is never read — block order is
    // push order — and the copper is engraved while the board is whole, flat and
    // undrilled: every hole made first is a place the surface can lift or the bit can
    // catch, and the engraving is the one operation whose quality is a depth tolerance.
    let mut blocks = Vec::new();
    if let Some((tool_id, depth)) = engraver.as_ref() {
        if let Some(bit) = ctx.tools.iter().find(|t| &t.id == tool_id) {
            let (spans, warnings) = plan_engrave_spans(ctx, raw, bit, *depth, &placement);
            notes.extend(warnings);
            blocks.extend(plan_engrave(
                &spans,
                tool_id,
                bit.diameter,
                placement.z_retract(),
                start,
                &slots,
            ));
        }
    }

    blocks.extend(plan_drilling(&drill_targets, &placement, start, &slots));
    blocks.extend(plan_routing(&route_targets, &placement, start, &slots));
    if let Some(outline_router) = routers.outline.as_deref() {
        let tool = ctx.tools.iter().find(|t| t.id == outline_router);
        if let Some(tool) = tool {
            let z_bottom = assigner::router_plunge(&setup);
            if tool
                .flute_length
                .is_some_and(|f| f.as_mm() < z_bottom.as_mm())
            {
                notes.push(format!(
                    "Outline router '{}' cannot reach through the board — the outline will \
                     not be cut free. Stock a longer cutter.",
                    tool.name
                ));
            }
            // The cutouts cut with this same tool ride in the same block, so a step that
            // routes both with one cutter pays one tool change rather than two.
            let mut spans = outline_spans.clone();
            spans.extend(cutout_spans.remove(outline_router).unwrap_or_default());
            blocks.extend(plan_outline(
                &spans,
                outline_router,
                tool.diameter,
                // Negative machine-Z depth (board top is Z0; op-planner §6).
                Length::from_mm(-z_bottom.as_mm()),
                placement.z_retract(),
                start,
                &slots,
            ));
        }
    }

    // Cutouts on any cutter the outline did not already use — one block each, since
    // `plan_outline` groups nothing across calls and two calls for one tool would read
    // as two tool changes.
    for (router_id, spans) in &cutout_spans {
        let Some(tool) = ctx.tools.iter().find(|t| &t.id == router_id) else {
            continue;
        };
        let z_bottom = assigner::router_plunge(&setup);
        if tool
            .flute_length
            .is_some_and(|f| f.as_mm() < z_bottom.as_mm())
        {
            notes.push(format!(
                "Cutout router '{}' cannot reach through the board — the openings will not \
                 be cut free. Stock a longer cutter.",
                tool.name
            ));
        }
        blocks.extend(plan_outline(
            spans,
            router_id,
            tool.diameter,
            Length::from_mm(-z_bottom.as_mm()),
            placement.z_retract(),
            start,
            &slots,
        ));
    }

    // The back-face program opens with a "Back face up?" prompt, and that
    // prompt is the *only* thing standing between a wrongly remounted board and a cut one:
    // two symmetric pins of one diameter accept the board unflipped or turned 180° just as
    // readily as the right way up. A controller with no `pause` primitive emits nothing for
    // it, so the guard silently is not there — which is worth saying out loud, before the
    // board is in the fixture rather than after.
    if raw.machines_back && cnc.pause_tpl.trim().is_empty() {
        notes.push(format!(
            "'{}' has no pause primitive, so this back-face program cannot ask the \
             operator to confirm the board was turned over. The locating pins are \
             symmetric and will accept it either way round — check it by eye before \
             running.",
            cnc.name,
        ));
    }

    // Record what this step's plan does not yet cover.
    if unmilled_slots > 0 {
        notes.push(format!(
            "{unmilled_slots} oblong slot(s) have no router narrow enough to mill, so their \
             route pass is missing — see the Tooling tab. Any drilling their strategy calls \
             for is planned."
        ));
    }
    if !short_flute_routers.is_empty() {
        notes.push(format!(
            "Slot router(s) {} cannot reach through the board — the slot walls will be cut \
             short. Stock a longer cutter.",
            short_flute_routers
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    // Surface unmillable slots here too: the Tooling tab carries the full detail.
    if !routers.unroutable_widths.is_empty() {
        notes.push(format!(
            "{} slot width(s) are narrower than any available router — see the Tooling tab.",
            routers.unroutable_widths.len()
        ));
    }
    for diagnostic in &assignment.diagnostics {
        notes.push(diagnostic.message.clone());
    }

    StepPlan {
        index,
        name,
        blocks,
        notes,
    }
}

/// Builds the board-outline cut spans, and pushes any mouse-bite holes onto
/// `drill_targets` so they are made while the board is still whole.
///
/// The pipeline, in the order the geometry demands:
///
/// 1. **Offset in board space** ([`pcb::routing_offset`]) — the kerf goes on the waste
///    side of each contour, so the board comes out at its drawn size. Board space, not
///    machine space, because the CNC's per-axis scaling would otherwise stretch a
///    constant kerf into a varying one.
/// 2. **Place** every offset point through the [`Placement`].
/// 3. **Split for tabs** — the job's own placements when it has any, otherwise the
///    profile's count spread evenly ([`outline::tab_positions`]). Measured on the offset
///    path, so a tab is the width asked for where the cutter actually passes.
/// 4. **Perforate**, when the retention mode asks for mouse bites.
///
/// Returns `Err` with an operator-facing reason when the outline cannot be cut at all.
/// Per-contour shortfalls — a contour that vanishes under the kerf, tabs the sides have
/// no room for — come back as warnings alongside the spans, because the rest of the
/// outline is still worth cutting.
/// How many net pairs a note names before it gives up and counts the rest.
///
/// A board laid out to one clearance rule narrows in dozens of places at once — 89 on the
/// board this was built against — and a step whose notes are ninety lines long is a step
/// whose notes nobody reads.
const NARROWED_PAIRS_NAMED: usize = 5;

/// The isolation cuts for this step's copper face, and what the operator should know.
///
/// Asks the [isolation worker](crate::runtime::isolation) rather than computing anything:
/// reading a board's copper and working out where a mill has to run takes seconds, and
/// this is called from a plan. When the answer for this exact question is not in hand yet
/// it asks for it and says so — the worker publishes through the context, which invalidates
/// the plan, which brings us back here with an answer. Asking twice is free, so there is no
/// guard around the request; see the worker's own note.
fn plan_engrave_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    bit: &Tool,
    depth: Length,
    placement: &Placement,
) -> (Vec<EngraveSpan>, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();
    let Some(board) = ctx.board.as_ref() else {
        return (Vec::new(), warnings);
    };

    // A mill reaches the surface, so the face the step machines decides the layer. The
    // placement already mirrors a back-face step, and copper arrives in the same board
    // coordinates the holes do, so B.Cu needs nothing further.
    let spec = IsolationSpec {
        board_name: board.name.clone(),
        board_epoch: ctx.board_epoch,
        layer_id: if raw.machines_back { pcb::BACK_COPPER } else { pcb::FRONT_COPPER },
        width_nm: (raw.engrave_copper.width.as_mm() * 1e6).round() as i64,
        // The bit's diameter is its tip — the narrowest channel it can cut — so it is
        // the floor of every width the pass may fall back to.
        min_width_nm: (bit.diameter.as_mm() * 1e6).round() as i64,
    };

    let Some(isolation) = ctx.isolation.matching(&spec) else {
        crate::runtime::isolation::request_isolation(spec);
        // Only a failure is worth a note. Work still in progress is not a shortcoming of
        // the job and does not belong in a list of them — the views say so themselves,
        // while it is happening, and stop saying it when it stops being true. A note
        // would have to be written now and would still be sitting there afterwards.
        if let Some(error) = ctx.isolation.error.as_ref() {
            warnings.push(format!("The copper could not be read: {error}"));
        }
        return (Vec::new(), warnings);
    };

    warnings.extend(isolation.copper_warnings.iter().cloned());
    warnings.extend(isolation.result.warnings.iter().cloned());
    if isolation.copper_layer_count > 2 {
        warnings.push(format!(
            "This board has {} copper layers and a mill reaches two of them. Only the \
             outer face is engraved; the inner layers are not made by this process at all.",
            isolation.copper_layer_count,
        ));
    }
    warnings.extend(narrowing_notes(ctx, &isolation.result.narrowed));

    let spans = isolation
        .result
        .contours
        .iter()
        .enumerate()
        .map(|(n, contour)| EngraveSpan {
            source: format!("{}#{n}", contour.net),
            path: contour
                .path
                .iter()
                .map(|&(x, y)| {
                    placement.xy(&pcb::BoardPoint {
                        x: Length::from_mm(x as f64 / 1e6),
                        y: Length::from_mm(y as f64 / 1e6),
                    })
                })
                .collect(),
            closed: contour.closed,
            // Negative machine-Z depth (board top is Z0; op-planner §6). Scaled from the
            // width this stretch actually achieved, not from the width that was asked for:
            // a narrowed stretch is a shallower cut, and that is the whole mechanism.
            z_bottom: Length::from_mm(-span_depth_mm(bit, contour.width_nm, depth)),
        })
        .collect();

    (spans, warnings)
}

/// How deep to sink `bit` to cut a channel `width_nm` across.
///
/// `full_depth` is what the requested width needs and what a uniform contour takes. A
/// narrowed stretch is re-derived from the width it actually got — the same arithmetic
/// that chose the bit, run again on a smaller number. That re-derivation *is* the
/// mechanism: on a V-bit a shallower cut is a narrower one, so the pass that decided to
/// squeeze through a tight gap is carried out by lifting the tool.
fn span_depth_mm(bit: &Tool, width_nm: i64, full_depth: Length) -> f64 {
    assigner::engrave_depth_mm(
        bit.diameter.as_mm(),
        bit.point_angle.as_degrees(),
        width_nm as f64 / 1e6,
    )
    .unwrap_or(full_depth.as_mm())
}

/// What to tell the operator about the pairs the board had no room for.
///
/// Two notes, because they are two different facts. A narrowed pair is *still isolated* —
/// worth knowing, not worth stopping for. A pair that got nothing is copper still joined,
/// which is a board that will not work, and it is named however many there are.
fn narrowing_notes(ctx: &AppCtx, narrowed: &[pcb::NarrowedPair]) -> Vec<String> {
    let mut notes = Vec::new();
    let name = |pair: &pcb::NarrowedPair| format!("{} / {}", pair.nets.0, pair.nets.1);

    let uncut: Vec<&pcb::NarrowedPair> = narrowed.iter().filter(|p| p.width_nm == 0).collect();
    if !uncut.is_empty() {
        notes.push(format!(
            "Not isolated — closer together than the narrowest cut this bit can make, so \
             no channel was cut between them and they stay joined: {}. Separate them by \
             hand, or re-lay the board.",
            uncut.iter().map(|p| name(p)).collect::<Vec<_>>().join(", "),
        ));
    }

    let tightened: Vec<&pcb::NarrowedPair> = narrowed.iter().filter(|p| p.width_nm > 0).collect();
    if let Some(tightest) = tightened.iter().min_by_key(|p| p.width_nm) {
        let named: Vec<String> = tightened
            .iter()
            .take(NARROWED_PAIRS_NAMED)
            .map(|p| format!("{} at {}", name(p), fmt_len(ctx, Length::from_mm(p.width_nm as f64 / 1e6))))
            .collect();
        let rest = tightened.len().saturating_sub(named.len());
        notes.push(format!(
            "{} net pair(s) are closer together than the requested channel, so it was \
             narrowed across those stretches only — the tightest to {}. {}{}",
            tightened.len(),
            fmt_len(ctx, Length::from_mm(tightest.width_nm as f64 / 1e6)),
            named.join(", "),
            if rest > 0 { format!(", and {rest} more.") } else { ".".into() },
        ));
    }
    notes
}

/// The cut spans for the board's interior openings, grouped by the router that cuts them,
/// plus the drills they need pushed onto `drill_targets`.
///
/// Separate from [`plan_outline_spans`] because a cutout is a different problem from a
/// boundary at every step. Its cutter is chosen by **fit** rather than matched to a
/// requested kerf, so two openings of different sizes may want two different cutters —
/// hence the grouping. The material it *removes* has to be held rather than the material
/// it leaves. And its corners are the ones a round cutter rounds off.
///
/// Both the corner relief and the mouse bites are drills, so they join the drill phase
/// and are made while the board is still whole and still registered — `Phase::Drill`
/// sorts before `Phase::Route`, so that ordering costs nothing here.
#[allow(clippy::too_many_arguments)]
fn plan_cutout_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    routers: &RouterPlan,
    placement: &Placement,
    drill_targets: &mut Vec<DrillTarget>,
    slots: &std::collections::BTreeMap<String, u8>,
    setup: &assigner::Setup,
) -> (
    std::collections::BTreeMap<String, Vec<OutlineSpan>>,
    Vec<String>,
) {
    let mut by_router: std::collections::BTreeMap<String, Vec<OutlineSpan>> = Default::default();
    let mut notes: Vec<String> = Vec::new();

    let Some(stitched) = ctx.stitched_board_data.as_ref() else {
        return (by_router, notes);
    };
    let cfg = &raw.route_cutouts;
    let placed_tabs = with_appdata(|data| data.job_edge_tabs());
    let mut corners_skipped = 0usize;

    // Numbered among the cutouts, as `job.yaml#/edge_tabs/index` means it.
    for (kind_index, (contour_index, contour)) in stitched
        .contours
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_hole)
        .enumerate()
    {
        let Some((router_id, fit)) = routers.cutouts.get(&contour_index) else {
            continue; // reported by the tooling plan as uncuttable
        };
        if !ctx.tools.iter().any(|t| &t.id == router_id) {
            continue; // chosen when the routers were planned, but no longer in stock
        }
        let label = TabContour::Cutout.as_str();

        let points: Vec<Point> = fit
            .wall_path
            .iter()
            .map(|&(x, y)| {
                placement.xy(&pcb::BoardPoint {
                    x: Length::from_mm(x as f64 / 1e6),
                    y: Length::from_mm(y as f64 / 1e6),
                })
            })
            .collect();
        let Some(path) = outline::Loop::new(&points) else {
            continue;
        };

        // --- corner relief, before anything is cut ---
        if cfg.drill_sharp_corners {
            for (n, corner) in pcb::convex_corners(contour, pcb::MIN_CORNER_TURN_RAD)
                .iter()
                .enumerate()
            {
                let s = (corner.interior_rad / 2.0).sin();
                if s <= f64::EPSILON {
                    continue;
                }
                // The drill was chosen and reserved a slot when the routers were planned,
                // so the two cannot disagree about which tool this corner uses.
                let Some(tool_id) = routers
                    .corner_drills
                    .get(&contour_index)
                    .and_then(|drills| drills.get(n))
                    .and_then(|d| d.clone())
                else {
                    corners_skipped += 1;
                    continue;
                };
                let Some(drill) = ctx.tools.iter().find(|t| t.id == tool_id) else {
                    corners_skipped += 1;
                    continue;
                };
                let (drill_dia, z_bottom) = (drill.diameter, assigner::router_plunge(setup));
                // Placed from the drill actually chosen, never from the ideal: tangency
                // is what keeps the hole inside the two edges, and a centre computed for
                // a larger drill would put a smaller one's cut past them.
                let offset_nm = (drill_dia.as_mm() / 2.0 / s) * 1e6;
                let at = pcb::BoardPoint {
                    x: Length::from_mm((corner.at.0 as f64 + corner.bisector.0 * offset_nm) / 1e6),
                    y: Length::from_mm((corner.at.1 as f64 + corner.bisector.1 * offset_nm) / 1e6),
                };
                drill_targets.push(DrillTarget {
                    source: format!("{label}#{kind_index}.corner{n}"),
                    at,
                    tool_id,
                    diameter: drill_dia,
                    z_bottom,
                    // One run per cutout so the relief holes are drilled round the
                    // opening in order rather than scattered through the tour.
                    chain: Some(format!("{label}#{kind_index}.corners")),
                });
            }
        }

        // --- the slug, and the tab that holds it ---
        let mut tabs: Vec<(f64, f64, bool)> = Vec::new(); // (fraction, width, bites)
        if cfg.retain_island {
            for (k, slug) in fit.slugs.iter().enumerate() {
                let perimeter_mm = pcb::path_perimeter_nm(slug) / 1e6;
                let slug_mm: Vec<(f64, f64)> = slug
                    .iter()
                    .map(|&(x, y)| (x as f64 / 1e6, y as f64 / 1e6))
                    .collect();
                let Some(anchor) = outline::longest_edge_midpoint(&slug_mm) else {
                    continue;
                };
                let anchor = placement.xy(&pcb::BoardPoint {
                    x: Length::from_mm(anchor.0),
                    y: Length::from_mm(anchor.1),
                });
                let Some(tab) = outline::island_tab(&path, perimeter_mm, anchor, cfg.tab_ratio)
                else {
                    continue;
                };
                // The operator's own nudge, keyed the way the job stores it.
                let nudge = placed_tabs
                    .iter()
                    .find(|t| {
                        t.contour == TabContour::Cutout && t.index == kind_index && t.tab == k
                    })
                    .map(|t| t.offset.as_mm())
                    .unwrap_or(0.0);
                tabs.push((
                    (tab.at + nudge / path.length_mm()).rem_euclid(1.0),
                    tab.width_mm,
                    tab.mouse_bites,
                ));
            }
        }

        // One width for the whole loop, since `cut_spans` cuts a single loop: the widest
        // of this cutout's tabs, so no island is held by less than it asked for.
        let width_mm = tabs.iter().map(|t| t.1).fold(0.0_f64, f64::max);
        let fractions: Vec<f64> = tabs.iter().map(|t| t.0).collect();

        for (n, span) in outline::cut_spans(&path, &fractions, width_mm)
            .into_iter()
            .enumerate()
        {
            by_router
                .entry(router_id.clone())
                .or_default()
                .push(OutlineSpan {
                    source: format!("{label}#{kind_index}.span{n}"),
                    path: span,
                });
        }

        for (n, (fraction, _, bites)) in tabs.iter().enumerate() {
            if !bites {
                continue;
            }
            let Some(bite_tool) = mouse_bite_drill(ctx, slots) else {
                continue;
            };
            for (h, centre) in outline::mouse_bite_centres(&path, *fraction, width_mm, bite_tool.1)
                .into_iter()
                .enumerate()
            {
                drill_targets.push(DrillTarget {
                    source: format!("{label}#{kind_index}.bite{n}.{h}"),
                    at: placement.unplace(&centre),
                    tool_id: bite_tool.0.clone(),
                    diameter: bite_tool.1,
                    z_bottom: bite_tool.2,
                    chain: Some(format!("{label}#{kind_index}.bite{n}")),
                });
            }
        }
    }

    if corners_skipped > 0 {
        notes.push(format!(
            "{corners_skipped} sharp corner(s) were left as the cutter rounds them: no drill \
             in the rack falls in the size band that would relieve them without leaving an \
             uncut web. Add a smaller drill to the toolset."
        ));
    }
    (by_router, notes)
}

fn plan_outline_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    routers: &RouterPlan,
    placement: &Placement,
    drill_targets: &mut Vec<DrillTarget>,
    slots: &std::collections::BTreeMap<String, u8>,
) -> Result<(Vec<OutlineSpan>, Vec<String>), String> {
    let Some(stitched) = ctx.stitched_board_data.as_ref() else {
        return Err(
            "The board outline has not been stitched yet — refresh the board snapshot.".into(),
        );
    };
    if !stitched.errors.is_empty() {
        return Err(format!(
            "The board outline could not be stitched into closed contours ({}), so it \
             cannot be routed.",
            stitched.errors.join("; ")
        ));
    }
    let Some(router_id) = routers.outline.as_deref() else {
        return Err(format!(
            "No {} router in stock, which is what the board's {} edge kerf needs. A kerf is              the cutter that makes it, so nothing else will cut it to size — stock that              cutter, or set the step's kerf to a size you have.",
            fmt_len(ctx, raw.route_board.kerf),
            fmt_len(ctx, raw.route_board.kerf),
        ));
    };
    let Some(router) = ctx.tools.iter().find(|t| t.id == router_id) else {
        return Err("The outline router is no longer in stock.".into());
    };

    let radius_nm = (router.diameter.as_mm() * 1e6 / 2.0).round() as i64;
    let offsets = pcb::routing_offset(&stitched.contours, radius_nm);

    // The job's placements, grouped the way they are stored: by contour kind and index.
    let placed_tabs = with_appdata(|data| data.job_edge_tabs());
    let edge = &raw.route_board;

    let mut spans: Vec<OutlineSpan> = Vec::new();
    let mut vanished = 0usize;
    // Tabs the outline had no room for, at the clearance the distribution keeps.
    let mut crowded = 0usize;
    // Interior openings this step leaves in the board, because nothing on it cuts them.
    let mut uncut_openings = 0usize;
    // Cutouts are numbered among themselves, as `job.yaml#/edge_tabs/index` means it.
    let (mut outer_n, mut cutout_n) = (0usize, 0usize);

    for (contour, offset) in stitched.contours.iter().zip(offsets) {
        let kind = if contour.is_hole {
            TabContour::Cutout
        } else {
            TabContour::Outer
        };
        let index = if contour.is_hole {
            &mut cutout_n
        } else {
            &mut outer_n
        };
        let (kind_index, label) = (*index, kind.as_str());
        *index += 1;

        // An interior opening is not this pass's to cut. `route_cutouts` owns them, with
        // a cutter chosen to fit each one rather than the kerf the boundary asked for.
        //
        // There was a `cutouts` flag on the edge operation that decided this, and so two
        // operations that could each cut the same opening; the flag is gone with it. A
        // board whose openings nothing is cutting is reported below rather than left to
        // come off the machine still solid in the middle.
        if contour.is_hole {
            uncut_openings += !raw.routes_cutouts() as usize;
            continue;
        }

        let Some(offset) = offset else {
            vanished += 1;
            continue;
        };
        let points: Vec<Point> = offset
            .iter()
            .map(|&(x, y)| {
                placement.xy(&pcb::BoardPoint {
                    x: Length::from_mm(x as f64 / 1e6),
                    y: Length::from_mm(y as f64 / 1e6),
                })
            })
            .collect();
        let Some(path) = outline::Loop::new(&points) else {
            continue;
        };

        let retention = edge.outline;
        let width_mm = retention.width.as_mm();

        // Where the tabs go. Distribution runs on the contour's own **straight
        // segments**, not on the offset polyline — the offset flattens every rounded
        // corner into dozens of chords, so "segments" there would be meaningless. Each
        // computed anchor is then placed and projected onto the offset path, which for
        // an outward offset of a straight run is exactly the perpendicular foot.
        let tabs: Vec<f64> = if retention.tabs {
            let anchors =
                outline::distribute_tabs(&straight_segments_mm(contour), retention.count, width_mm);
            if anchors.len() < retention.count {
                crowded += retention.count - anchors.len();
            }
            anchors
                .iter()
                .enumerate()
                .map(|(n, anchor)| {
                    let at = path.nearest_fraction(placement.xy(&pcb::BoardPoint {
                        x: Length::from_mm(anchor.point.0),
                        y: Length::from_mm(anchor.point.1),
                    }));
                    // The operator's own nudge, as a fraction of the loop.
                    let nudge = placed_tabs
                        .iter()
                        .find(|t| t.contour == kind && t.index == kind_index && t.tab == n)
                        .map(|t| t.offset.as_mm())
                        .unwrap_or(0.0);
                    (at + nudge / path.length_mm()).rem_euclid(1.0)
                })
                .collect()
        } else {
            Vec::new()
        };

        for (n, span) in outline::cut_spans(&path, &tabs, width_mm)
            .into_iter()
            .enumerate()
        {
            spans.push(OutlineSpan {
                source: format!("{label}#{kind_index}.span{n}"),
                path: span,
            });
        }

        // Mouse bites are drills, so they join the drill phase rather than the route one.
        if retention.mouse_bites {
            if let Some(bite_tool) = mouse_bite_drill(ctx, slots) {
                for (n, tab) in tabs.iter().enumerate() {
                    let centres = outline::mouse_bite_centres(&path, *tab, width_mm, bite_tool.1);
                    for (h, centre) in centres.into_iter().enumerate() {
                        drill_targets.push(DrillTarget {
                            source: format!("{label}#{kind_index}.bite{n}.{h}"),
                            // The span geometry is already placed, so unplace it: the
                            // drill planner places its own targets.
                            at: placement.unplace(&centre),
                            tool_id: bite_tool.0.clone(),
                            diameter: bite_tool.1,
                            z_bottom: bite_tool.2,
                            // One run, so the perforation is drilled in order along the
                            // tab rather than being scattered through the tour.
                            chain: Some(format!("{label}#{kind_index}.bite{n}")),
                        });
                    }
                }
            }
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    if uncut_openings > 0 {
        warnings.push(format!(
            "{uncut_openings} interior opening(s) are left in the board: the edge pass cuts \
             the boundary only. Add 'Route interior cutouts' to this step to cut them, each \
             with a cutter chosen to fit."
        ));
    }
    if crowded > 0 {
        warnings.push(format!(
            "{crowded} retaining tab(s) could not be placed: the outline's straight sides \
             have no room left at the required clearance. Widen the board's sides, narrow \
             the tabs, or ask for fewer."
        ));
    }
    if vanished > 0 {
        return Err(format!(
            "{vanished} outline contour(s) are smaller than the {} router and vanish under \
             its kerf, so they cannot be cut. Stock a smaller cutter.",
            router.name
        ));
    }
    Ok((spans, warnings))
}

/// A contour's straight sides as `(x0, y0, x1, y1)` in board millimetres — the only
/// segments a tab may sit on.
///
/// Arcs and beziers are skipped. A tab on a curve is one the operator has to snap on a
/// radius, and the distribution's even-spacing and clearance arithmetic is stated in
/// straight-line lengths. A rounded-corner board therefore takes its tabs on the flats,
/// which is where they belong anyway.
fn straight_segments_mm(contour: &pcb::Contour) -> Vec<(f64, f64, f64, f64)> {
    contour
        .segments
        .iter()
        .filter_map(|segment| match *segment {
            pcb::Segment::Line { start, end } => Some((
                start.0 as f64 / 1e6,
                start.1 as f64 / 1e6,
                end.0 as f64 / 1e6,
                end.1 as f64 / 1e6,
            )),
            _ => None,
        })
        .collect()
}

/// The drill that perforates a mouse bite: `(tool id, diameter, plunge)`.
///
/// The smallest drill already in the rack, because a mouse bite wants the smallest hole
/// that will still break cleanly and — more to the point — must not add a tool change of
/// its own to a step that has already been assigned. `None` when the rack holds no drill,
/// which leaves the tab solid rather than inventing a tool.
fn mouse_bite_drill(
    ctx: &AppCtx,
    slots: &std::collections::BTreeMap<String, u8>,
) -> Option<(String, Length, Length)> {
    let setup = build_setup(ctx, None);
    ctx.tools
        .iter()
        .filter(|t| slots.contains_key(&t.id) && !t.kind.eq_ignore_ascii_case("router"))
        .min_by_key(|t| t.diameter.as_um().round() as i64)
        .map(|t| (t.id.clone(), t.diameter, assigner::router_plunge(&setup)))
}

/// How deep a locating-pin hole goes: **through** the board and on into the backboard, by
/// the whole of the space below the board that the fixture says is usable.
///
/// Deliberately not routed through [`assigner::assign`]'s Z-feasibility check, which would
/// reject this by construction. That check exists to stop a tool reaching the machine bed,
/// and it measures the room left below the board — the very room a pin hole is *supposed*
/// to consume. A pin that only engages the board is not registration: the board pivots on
/// it. What keeps the bed safe here is that the engagement stops at the fixture's own
/// `bed_clearance`, which is where [`build_setup`] has already subtracted it.
///
/// (`Setup::bed_clearance` is that remaining space, not the clearance itself — see
/// [`build_setup`].)
fn pin_plunge(setup: &assigner::Setup) -> Length {
    Length::from_mm(setup.board_thickness.as_mm() + setup.bed_clearance.as_mm())
}

/// The least a pin may engage the backboard before it stops holding the board square.
const MIN_PIN_ENGAGEMENT_MM: f64 = 1.0;

/// The achieved engagement when it is too shallow to rely on, or `None` when it is fine.
///
/// A warning and not a refusal: a shallow pin still registers a board that is held down by
/// something else, and the operator is the one who can see how their backboard is set up.
/// Refusing here would block a job that works.
///
/// Note this is the depth the *tip* reaches, which is what the fixed rule specifies. A
/// drill's point is conical, so the full-diameter part of the hole — the part the pin
/// actually seats in — is shorter than this by the point length (~1 mm for a 118° ⌀3.2).
fn shallow_pin_engagement(setup: &assigner::Setup) -> Option<Length> {
    (setup.bed_clearance.as_mm() < MIN_PIN_ENGAGEMENT_MM).then_some(setup.bed_clearance)
}

/// Formats a length in the operator's preferred unit.
fn fmt_len(ctx: &AppCtx, length: Length) -> String {
    length.unit_display(ctx.unit_system).user
}

/// A step that could not be planned — no blocks, the reasons surfaced as notes.
fn failed(index: usize, name: String, messages: Vec<String>) -> StepPlan {
    StepPlan {
        index,
        name,
        blocks: vec![],
        notes: messages,
    }
}

/// A compact one-liner per assigner error; the Tooling tab carries the full detail.
fn format_assign_error(error: &AssignError) -> Vec<String> {
    match error {
        AssignError::UncoverableHoles(faults) => vec![format!(
            "{} hole requirement(s) have no usable tool — see the Tooling tab.",
            faults.len()
        )],
        AssignError::RackTooSmall { minimal, capacity } => vec![format!(
            "Rack too small: needs {minimal} tools but {capacity} usable slot(s) — see the Tooling tab."
        )],
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn key(context: u64, data: u64) -> PlanKey {
        PlanKey { context, data }
    }

    fn marker(note: &str) -> MachiningPlan {
        MachiningPlan { steps: Vec::new(), note: Some(note.to_string()) }
    }

    /// The reason the cache exists: the views re-render for reasons that have nothing to
    /// do with the job, and planning a dense board is expensive enough to be felt.
    #[test]
    fn a_repeated_ask_for_the_same_job_plans_once() {
        let cache = PlanCache::default();
        let builds = AtomicUsize::new(0);
        let build = || {
            builds.fetch_add(1, Ordering::SeqCst);
            marker("planned")
        };

        let first = cache.get_or_build(key(1, 1), build);
        let second = cache.get_or_build(key(1, 1), build);

        assert_eq!(builds.load(Ordering::SeqCst), 1, "the second ask must not re-plan");
        assert!(Arc::ptr_eq(&first, &second), "and must hand back the very same plan");
    }

    /// Either store moving is a job that may have changed. Counting them separately is
    /// what catches an edit made straight into the datastore, which never touches the
    /// context and so would leave its revision standing still.
    #[test]
    fn a_move_in_either_store_re_plans() {
        let cache = PlanCache::default();
        cache.get_or_build(key(1, 1), || marker("first"));

        let after_context = cache.get_or_build(key(2, 1), || marker("second"));
        assert_eq!(after_context.note.as_deref(), Some("second"));

        let after_data = cache.get_or_build(key(2, 2), || marker("third"));
        assert_eq!(after_data.note.as_deref(), Some("third"));
    }

    /// The property the whole design rests on. A clone of the context is a *snapshot*,
    /// taken on every render; if each carried its own cache the hit rate would be zero
    /// and this would be an elaborate way to plan exactly as often as before.
    #[test]
    fn a_cloned_context_shares_the_cache_rather_than_copying_it() {
        let cache = PlanCache::default();
        let first = cache.get_or_build(key(1, 1), || marker("planned"));

        let snapshot = cache.clone();
        let builds = AtomicUsize::new(0);
        let second = snapshot.get_or_build(key(1, 1), || {
            builds.fetch_add(1, Ordering::SeqCst);
            marker("re-planned")
        });

        assert_eq!(builds.load(Ordering::SeqCst), 0, "the snapshot must see the same cell");
        assert!(Arc::ptr_eq(&first, &second));
    }

    /// A stale snapshot must not be able to file its plan under the current revision.
    /// The revision is a field carried by the snapshot for exactly this reason, and if it
    /// ever became a global read at call time this test is what would fail.
    #[test]
    fn a_stale_snapshot_cannot_pass_its_plan_off_as_current() {
        let cache = PlanCache::default();
        cache.get_or_build(key(1, 1), || marker("from the old snapshot"));

        let current = cache.get_or_build(key(2, 1), || marker("from the current one"));
        assert_eq!(current.note.as_deref(), Some("from the current one"));
    }
}
