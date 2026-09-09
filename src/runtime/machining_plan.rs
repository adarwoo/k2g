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

use crate::data::model::tool_core::ToolKind;
use crate::data::model::{FixtureProfile, TabContour, Tool};
use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{
    self, engrave_depth_per_width, AssignConfig, AssignError, Strategy, Weights,
};
use crate::gcode::placement::{BoardFlip, BoardOrigin, Margin, Placement, PlacementSpec};
use crate::gcode::plan::{MachiningPlan, Point, StepPlan, VerifyStop};
use crate::gcode::planner::{
    plan_drilling, plan_engrave, plan_outline, plan_routing, DrillTarget, EngraveSpan, OutlineSpan,
    RouteShape, RouteTarget, TestCut,
};
use crate::gcode::{oblong, outline, pins, scene, testcut};
use crate::runtime::isolation::IsolationSpec;
use crate::runtime::tooling::{
    build_rack_spec, build_setup, collect_hole_groups, missing_bindings, pick_engraver,
    pick_pin_tool, plan_routers, read_steps, EngraveChoice, HoleGroup, PenetrationBudget, PinTool,
    RouterPlan, StepRaw,
};
use crate::runtime::AppCtx;

/// The coordinate frame every program of one job is written in.
///
/// Derived once per job (see the module note) so the steps cannot disagree about where the
/// zero is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JobFrame {
    /// The finished offset from the board's bounding box to the work origin: the widest thing
    /// the job cuts outside the board, plus the fixture's work clearance. See [`job_frame`].
    pub margin: Margin,
    /// The fixture's declared work clearance, kept apart from [`Self::margin`] because it is the
    /// operator's own number and the notes quote it back to them.
    pub clearance: Margin,
    /// The pin diameter, when the job drills pins at all.
    pub pin_diameter: Option<Length>,
    /// Which axis the board turns about, from the fixture. Meaningful for the pins even on
    /// an all-front job, because it decides which pair of sides they sit on.
    pub flip_axis: BoardFlip,
    /// Which corner of the bed the zero sits on, from the fixture.
    ///
    /// Here rather than read per step because the *margin* depends on it — the test cut's band
    /// and the clearance are claimed on the origin's two sides — and a margin computed against
    /// one corner while the geometry is placed against another would open the band on the wrong
    /// edges. Like [`Self::flip_axis`], it is a fact about where the registration is.
    pub origin: BoardOrigin,
    /// How wide a band the engraving depth test cut is cut in, when any step asks for one.
    ///
    /// Resolved here, not in the step, because it is one of the extents the frame is built from
    /// — and because a test cut in step 1 moves the **one** zero step 3's drilling is written
    /// against too. That is not a leak; it is what one frame means.
    pub test_band: Option<Length>,
    /// The material the outline routing removes outside the board (`kerf + finishing`), zero
    /// when no step cuts the board out.
    ///
    /// Kept so the notes can say whether the test cut reused this band or had one opened for
    /// it — the difference between costing nothing and moving every coordinate in the job.
    ///
    /// `Option` only because [`Length`] has no `Default` and this type derives one; `None` and
    /// zero mean the same thing — no step cuts the board out.
    pub waste: Option<Length>,
}

/// The job's shared frame, from the profile's steps and the fixture holding the board.
///
/// The pin diameter is taken from **the first step that drills pins**, which the readiness
/// gate has already established is step 1 if it exists at all
/// ([`locating_pin_faults`](crate::runtime::tooling::locating_pin_faults)). Taking the
/// first rather than, say, the largest is the honest reading of "the pins this job is
/// registered by": a second pin step re-fixtures against the same holes.
///
/// The flip axis, the origin corner and the work clearance come from the fixture rather than
/// from a step because they are facts about where the registration *is*, which the fixture owns.
///
/// # Extents, then the clearance
///
/// The fixture declares `work_clearance` — how close a cutting tool's **edge** may come to the
/// zero. Everything the job cuts outside the board declares an **extent**: how far it reaches
/// from the board's bounding box, stated without needing to know where the board is, which is
/// what breaks the circularity (the origin clears the work, the work is measured from the placed
/// board, the placed board depends on the origin). Three claim extents today:
///
/// | claimant | extent from the bbox | sides | read back out by |
/// |---|---|---|---|
/// | routed outline | `kerf + finishing` — the material it removes | all four | — (it is waste) |
/// | locating pins | `1.5 × diameter` | the flip axis' two | [`pins::centres`] |
/// | depth test cut | [`testcut::band`] | the origin's two | [`testcut::l_path`] |
///
/// The extents combine with [`Margin::widest`] and **not** [`Margin::stack`]: they are all
/// measured from the same edge and overlap in the material, so summing them would charge the
/// frame twice for one piece of blank and push the board further out than anything needs.
/// Whichever is widest ends up with its cutting edge exactly on the clearance line.
///
/// The clearance is then stacked *outside* the widest of them, because it is measured from the
/// zero rather than from the board. `widest` there would let a wide extent swallow it whole and
/// put a cutting edge on the origin.
///
/// A fourth claimant adds one line to the array and needs nothing else.
fn job_frame(steps: &[StepRaw], fixture: Option<&FixtureProfile>) -> JobFrame {
    let flip_axis = fixture
        .map(|f| BoardFlip::from_axis(&f.board_flip_axis))
        .unwrap_or(BoardFlip::AboutY);
    let origin = fixture
        .map(|f| BoardOrigin::from_edges(&f.origin_x0, &f.origin_y0))
        .unwrap_or_default();
    let clearance = fixture
        .map(|f| Margin {
            x_min: if origin.x_at_right { 0.0 } else { f.work_clearance_x.as_mm() },
            x_max: if origin.x_at_right { f.work_clearance_x.as_mm() } else { 0.0 },
            y_min: if origin.y_at_far { 0.0 } else { f.work_clearance_y.as_mm() },
            y_max: if origin.y_at_far { f.work_clearance_y.as_mm() } else { 0.0 },
        })
        .unwrap_or_default();

    let pin_diameter = steps
        .iter()
        .find(|step| step.drills_locating_pins())
        .and_then(|step| step.pin_diameter);

    // The material the outline router removes, from whichever step routes it. Nominal — the
    // finishing allowance `finishing_allowance` may zero out per step is still counted here,
    // because a frame that is a tenth of a millimetre generous costs nothing and a frame that is
    // a tenth short puts a cutting edge inside the clearance.
    let waste = steps
        .iter()
        .filter(|step| step.routes_outline() && step.route_board.cuts_through())
        .map(|step| step.route_board.kerf.as_mm() + step.route_board.finishing.as_mm())
        .fold(0.0f64, f64::max);

    // The band the test cut is made in — the routed waste when it is wide enough, otherwise one
    // opened for it. The trough is the width the operator *asked for*: this has to be answerable
    // before a V-bit is picked, for the same reason the pin margin has to be answerable before
    // the board is placed.
    let test_band = steps
        .iter()
        .filter(|step| step.engraves_copper() && step.engrave_copper.test_cut)
        .map(|step| testcut::band(Length::from_mm(waste), step.engrave_copper.width))
        .fold(None::<Length>, |acc, b| {
            Some(acc.map_or(b, |a| if a.as_mm() >= b.as_mm() { a } else { b }))
        });

    let extents = [
        pin_diameter.map(|d| pins::margin(d, flip_axis)),
        (waste > 0.0).then(|| Margin::uniform(waste)),
        test_band.map(|b| testcut::margin(b, origin)),
    ]
    .into_iter()
    .flatten()
    .fold(Margin::default(), Margin::widest);

    JobFrame {
        margin: extents.stack(clearance),
        clearance,
        pin_diameter,
        flip_axis,
        origin,
        test_band,
        waste: (waste > 0.0).then(|| Length::from_mm(waste)),
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
    // The depth is the copper plus a penetration into the laminate beneath it, so the bit
    // cannot be chosen without knowing how much copper there is. KiCad's stackup says, per
    // face; when it does not, one ounce is assumed and the step says so below.
    let budget = PenetrationBudget::current();
    let (copper, copper_assumed) =
        crate::runtime::tooling::copper_thickness(ctx.board.as_ref(), raw.machines_back);
    let engraver = if raw.engraves_copper() {
        match pick_engraver(&ctx.tools, toolset, raw.engrave_copper.width, copper, budget) {
            Some(picked) => Some(picked),
            None => {
                return failed(index, name, vec![crate::runtime::tooling::no_engraver_reason()])
            }
        }
    } else {
        None
    };
    // A bit that falls short of the requested width machines the board and says so. The
    // note goes on before anything else the step has to say, because it is the one thing
    // here that means the board will not meet the specification it was asked for.
    if let Some(choice) = engraver.as_ref() {
        if let Some(wanted) = choice.fell_short_of {
            notes.push(crate::runtime::tooling::narrower_channel_reason(
                ctx,
                wanted,
                choice.width,
                copper,
                budget,
            ));
        }
    }

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
    if let Some(choice) = engraver.as_ref() {
        mandatory.push(choice.tool_id.clone());
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
                        is_pin: false,
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
                is_pin: false,
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
    // This step's own fixture, not the frame's: which *corner* the zero sits on is a fact about
    // the fixture this step is actually set up in, and a step is a whole physical setup, so a
    // second one on a different fixture legitimately zeroes elsewhere.
    //
    // Note this is the one thing the frame's margin depends on that is read per step
    // (`frame.origin` is the *first* step's). They agree in every single-fixture profile, which
    // is all of them that engrave. Where they do not, the depth test cut's band is opened on
    // the wrong two edges and `testcut::l_path` refuses rather than misplacing the cut — see
    // `plan_test_cut`, which says so instead of going quiet.
    let board_origin = BoardOrigin::from_edges(&fixture.origin_x0, &fixture.origin_y0);
    let placement = Placement::new(&PlacementSpec {
        bounds: ctx.board.as_ref().and_then(|b| b.bounding_box.as_ref()),
        orientation_deg: orientation,
        origin: board_origin,
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

    // **Where the operator's zero actually goes.** Said every time, because until now it was
    // said nowhere at all: the origin is a computed point out in bare blank with no stop, no pin
    // and no witness mark at it, and it moves whenever anything the job cuts outside the board
    // changes. An operator who could not find it had nothing to go on — the schema told them it
    // was the corner the board is registered into, which it has not been since the origin
    // started making room for things.
    //
    // Two numbers, and they are different on the two axes whenever the pins or the routing
    // reach further on one than the other. The clearance is quoted separately because it is the
    // operator's own figure and the rest is what the job added to it.
    if ctx.board.is_some() {
        let rect = placement.board_rect_mm();
        let (dx, dy) = (rect.min_x.abs().min(rect.max_x.abs()), rect.min_y.abs().min(rect.max_y.abs()));
        notes.push(format!(
            "Set the work origin {} from the board in X and {} in Y — your {} / {} work \
             clearance plus the room this job's cuts outside the board need. Nothing is \
             machined nearer the zero than the clearance.",
            fmt_len(ctx, Length::from_mm(dx)),
            fmt_len(ctx, Length::from_mm(dy)),
            fmt_len(ctx, Length::from_mm(frame.clearance.x_min.max(frame.clearance.x_max))),
            fmt_len(ctx, Length::from_mm(frame.clearance.y_min.max(frame.clearance.y_max))),
        ));
    }

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
                    is_pin: true,
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
    let mut outline_rough: Vec<OutlineSpan> = Vec::new();
    let mut outline_spans: Vec<OutlineSpan> = Vec::new();
    if has_route {
        if raw.route_board.cuts_through() {
            match plan_outline_spans(ctx, raw, &routers, &placement, &mut drill_targets, &slots) {
                Ok(passes) => {
                    notes.extend(passes.warnings);
                    if passes.finish.is_empty() {
                        notes.push(
                            "The retaining tabs are wider than the outline they sit on, so \
                             nothing would be cut. Reduce the tab width or the tab count."
                                .into(),
                        );
                    }
                    outline_rough = passes.rough;
                    outline_spans = passes.finish;
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
    let mut cutout_rough: SpansByRouter = Default::default();
    let mut cutout_spans: SpansByRouter = Default::default();
    if raw.routes_cutouts() {
        let passes = plan_cutout_spans(
            ctx,
            raw,
            &routers,
            &placement,
            &mut drill_targets,
            &slots,
            &setup,
        );
        notes.extend(passes.warnings);
        cutout_rough = passes.rough;
        cutout_spans = passes.finish;
    }

    // The isolation pass, first of all the blocks. `Phase` is never read — block order is
    // push order — and the copper is engraved while the board is whole, flat and
    // undrilled: every hole made first is a place the surface can lift or the bit can
    // catch, and the engraving is the one operation whose quality is a depth tolerance.
    let mut blocks = Vec::new();
    if let Some(choice) = engraver.as_ref() {
        if let Some(bit) = ctx.tools.iter().find(|t| t.id == choice.tool_id) {
            if copper_assumed {
                notes.push(format!(
                    "KiCad's stackup states no copper thickness for this face, so {} (1 oz) \
                     was assumed. The isolation depth is that plus the substrate \
                     penetration, and the channel width follows from it — set the stackup \
                     if the board is not 1 oz.",
                    fmt_len(ctx, copper),
                ));
            }
            let (spans, warnings) = plan_engrave_spans(ctx, raw, bit, choice, &placement);
            notes.extend(warnings);
            let (test_cut, test_notes) =
                plan_test_cut(ctx, raw, cnc, bit, choice, &placement, frame);
            notes.extend(test_notes);
            let engraved = plan_engrave(
                &spans,
                test_cut,
                &choice.tool_id,
                bit.diameter,
                placement.z_retract(),
                start,
                &slots,
            );
            // **A step that asks to engrave and produces no engraving is a fault**, not a
            // step that happened to be quiet. Nothing checked this, which is how a program
            // came out drilled, routed and with its copper untouched while every view
            // showed it as complete. Whatever the reason — contours still computing, a
            // board with no copper on this face — the reason is in `notes` by now; this is
            // what stops the plan looking finished without it being read.
            if engraved.is_none() {
                notes.push(
                    "This step engraves copper but produced no isolation toolpath — the \
                     program will not separate the nets. Do not run it as it is."
                        .to_string(),
                );
            }
            blocks.extend(engraved);
        }
    }

    blocks.extend(plan_drilling(&drill_targets, &placement, start, &slots));
    blocks.extend(plan_routing(&route_targets, &placement, start, &slots));

    // The interior openings, on every cutter the outline does not itself use — **before**
    // the outline block, because the perimeter is what releases the part and nothing
    // should be machined on a board that has already been let go of (op-planner §4). One
    // block each, since `plan_outline` groups nothing across calls and two calls for one
    // tool would read as two tool changes.
    //
    // The cutter the outline *does* use is taken out of these maps first and cut inside
    // that block, as its leading passes.
    let shared_cutout_rough = routers
        .outline
        .as_deref()
        .and_then(|id| cutout_rough.remove(id))
        .unwrap_or_default();
    let shared_cutout_spans = routers
        .outline
        .as_deref()
        .and_then(|id| cutout_spans.remove(id))
        .unwrap_or_default();

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
            &[
                cutout_rough.get(router_id).map(Vec::as_slice).unwrap_or_default(),
                spans,
            ],
            router_id,
            tool.diameter,
            Length::from_mm(-z_bottom.as_mm()),
            placement.z_retract(),
            start,
            &slots,
        ));
    }

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
            // routes both with one cutter pays one tool change rather than two — but as
            // their **own passes, ahead of the outline's**.
            //
            // They used to be merged into the outline's two pass lists, on the argument
            // that joining the roughing pass kept them ahead of the perimeter's finishing
            // cut and so kept op-planner §4's "interior before perimeter" true. It did not.
            // `Passes::rough` is empty whenever the step leaves no finishing allowance —
            // the common case — so every cutout and the whole perimeter landed in one pass
            // ordered by travel, and a cutout could be machined after the perimeter had
            // been cut through. Separate passes make the rule hold whether or not there is
            // an allowance, which is what it was always supposed to mean.
            //
            // `plan_outline` runs its passes in order and tours each from where the last
            // left the cutter, so this is still one block and costs no tool change.
            blocks.extend(plan_outline(
                &[
                    &shared_cutout_rough,
                    &shared_cutout_spans,
                    &outline_rough,
                    &outline_spans,
                ],
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

    // The back-face program opens with a "Back face up?" prompt, and that prompt is the
    // *only* thing standing between a wrongly mounted board and a cut one. A controller
    // with no `pause` primitive emits nothing for it, so the guard silently is not there —
    // which is worth saying out loud, before the board is in the fixture rather than after.
    //
    // Two different risks, because the first step is not registered against anything. On a
    // later step the board is on the pins, and the danger is that they accept it: two
    // symmetric holes of one diameter take the board unflipped or turned 180° just as
    // readily as the right way up. On the first step there are no pins yet — the board is
    // held however the fixture holds it — so the danger is simply a blank loaded the wrong
    // way up, with nothing at all to catch it.
    if raw.machines_back && cnc.pause_tpl.trim().is_empty() {
        notes.push(if index == 0 {
            format!(
                "'{}' has no pause primitive, so this program cannot ask the operator to \
                 confirm the board is back-face up. It is the first step, so nothing \
                 registers the board yet — check the blank is the right way up before \
                 running.",
                cnc.name,
            )
        } else {
            format!(
                "'{}' has no pause primitive, so this back-face program cannot ask the \
                 operator to confirm the board was turned over. The locating pins are \
                 symmetric and will accept it either way round — check it by eye before \
                 running.",
                cnc.name,
            )
        });
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
/// 5. **Rough, then finish**, when the step leaves a finishing allowance: the same loop
///    taken one allowance further onto the waste side and cut conventional, then the loop
///    that makes the edge, cut climb. The two come back separately because they must reach
///    the machine in that order — see [`plan_outline`].
///
/// Returns `Err` with an operator-facing reason when the outline cannot be cut at all.
/// Per-contour shortfalls — a contour that vanishes under the kerf, tabs the sides have
/// no room for, an allowance that cannot be honoured — come back as warnings alongside the
/// spans, because the rest of the outline is still worth cutting.
/// How many net pairs a note names before it gives up and counts the rest.
///
/// A board laid out to one clearance rule narrows in dozens of places at once — 89 on the
/// board this was built against — and a step whose notes are ninety lines long is a step
/// whose notes nobody reads.
const NARROWED_PAIRS_NAMED: usize = 5;

/// Says which part of the isolation question is unanswered, field by field.
///
/// A miss is normal — the first plan after a board loads always misses, and the answer
/// lands a second later. It is a *persistent* miss that is the fault, and from the outside
/// the two look identical: no engraving, and a plan that keeps asking. The spec is five
/// fields and any one of them can differ, so this names the one that does rather than
/// leaving it to be guessed at from a screenshot.
///
/// At `info` deliberately. It is not a warning — nothing is wrong the first several times —
/// but it has to be in the log the operator can hand over, not behind `RUST_LOG=debug`
/// nobody sets before the fault rather than after it.
fn log_isolation_miss(held: &crate::runtime::isolation::IsolationState, wanted: &IsolationSpec) {
    match held.ready.get(&wanted.layer_id) {
        None => log::info!(
            "Isolation not ready for layer {}: nothing held for this face yet \
             (board {:?} epoch {}, width {} nm, floor {} nm)",
            wanted.layer_id,
            wanted.board_name,
            wanted.board_epoch,
            wanted.width_nm,
            wanted.min_width_nm,
        ),
        Some(held) => {
            let held = &held.spec;
            let mut differs: Vec<String> = Vec::new();
            if held.board_name != wanted.board_name {
                differs.push(format!("board {:?} != {:?}", held.board_name, wanted.board_name));
            }
            if held.board_epoch != wanted.board_epoch {
                differs.push(format!("epoch {} != {}", held.board_epoch, wanted.board_epoch));
            }
            if held.width_nm != wanted.width_nm {
                differs.push(format!("width {} != {} nm", held.width_nm, wanted.width_nm));
            }
            if held.min_width_nm != wanted.min_width_nm {
                differs.push(format!(
                    "floor {} != {} nm",
                    held.min_width_nm, wanted.min_width_nm
                ));
            }
            log::info!(
                "Isolation not ready for layer {}: held answers a different question — {}",
                wanted.layer_id,
                match differs.is_empty() {
                    // Equal on every field this compares, yet `matching` refused it: the
                    // spec has grown a field and this function was not told.
                    true => "nothing this check knows about (a spec field is unaccounted \
                             for here)"
                        .to_string(),
                    false => differs.join(", "),
                },
            );
        }
    }
}

/// What is said to the operator at the depth-test stop.
///
/// **The measurement and the correction are in different quantities**, and this is what
/// bridges them. They read a *width* off the groove — a channel 50 µm deep has no depth anyone
/// can get a gauge into, while its width sits under a loupe next to a scale — but the dial they
/// turn is *Z*. Without the conversion, "0.05 mm too wide" is a fault with no remedy and the
/// whole exercise ends in a guess at the one number it existed to produce.
///
/// [`engrave_depth_per_width`] is exact rather than a local slope, and depends only on the cone,
/// so it can be stated once and scaled to whatever error is found. It is given both ways — the
/// multiplier, and what 0.10 mm of width is worth — because one of the two is always the easier
/// arithmetic to do standing at a machine.
///
/// Pure, and split out from [`plan_test_cut`] so it can be tested without a context: these
/// sentences are the interface to a hand on a Z dial, and their arithmetic is worth pinning.
///
/// **Millimetres, always, and never through `fmt_len`.** That follows the operator's *display*
/// preference, and a program whose text changed because someone switched the UI to inches would
/// not be the deterministic output the planner promises. Three decimals, not two: the whole
/// subject is tens of microns, and a channel quoted as "0.16 mm" cannot be measured against.
///
/// No parentheses anywhere — these go out through the machine's `comment` primitive, which every
/// bundled profile renders as `( {text} )`.
fn test_cut_advice(width: Length, depth: Length, point_angle_deg: f64) -> Vec<String> {
    let mut advice = vec![
        "Depth test cut - an L in the waste, cut at the isolation depth.".to_string(),
        format!(
            "The channel should measure {:.3} mm across, {:.3} mm deep.",
            width.as_mm(),
            depth.as_mm(),
        ),
    ];

    match engrave_depth_per_width(point_angle_deg) {
        Some(ratio) => {
            advice.push(format!(
                "Width error x {ratio:.2} = the Z correction: 0.10 mm out is {:.3} mm of Z.",
                0.1 * ratio,
            ));
            advice.push(
                "Too narrow means too shallow - lower Z by that, reset and run again."
                    .to_string(),
            );
            advice.push(
                "Too wide means too deep - raise Z by that, shift the work offset in X and Y \
                 onto fresh material, reset and run again."
                    .to_string(),
            );
        }
        // A flat-tipped tool cuts one width however deep it goes, so the groove's width says
        // nothing at all about Z. Offering a conversion here would be inventing one; say what
        // there is to go on instead.
        None => {
            advice.push(
                "This tool cuts one width at any depth, so judge the cut itself. Too faint \
                 means too shallow - lower Z, reset and run again."
                    .to_string(),
            );
            advice.push(
                "Through to the substrate means too deep - raise Z, shift the work offset in \
                 X and Y onto fresh material, reset and run again."
                    .to_string(),
            );
        }
    }

    advice
}

/// The **depth test cut** for this step, when it asked for one, and what to say about it.
///
/// Engraving is the one operation whose quality is a depth tolerance, cut at one fixed Z with
/// no probing anywhere in the product to check that Z0 is the board surface. This is the manual
/// equivalent: an L in the waste at exactly the depth the pass will use, and a stop so it can
/// be looked at before the copper is touched. See [`crate::gcode::testcut`] for the geometry.
///
/// Returns `None` in three cases, two of which say why:
///
/// - the step did not ask for one — silent, there is nothing to report;
/// - **the machine has no `pause` word.** The L's only output is a decision made at the stop, so
///   without the stop it is a groove cut in a corner nothing has checked, followed by the copper
///   being engraved at the very depth that was not verified. Every branch is worse than not
///   cutting, so the cut is dropped rather than degraded — the same call `pick_engraver` makes
///   when there is no degraded output worth having;
/// - the placement made no room for it, which is [`testcut::l_path`]'s own guard and can only
///   mean the frame and this step disagree about whether there is a test cut.
///
/// The L runs down the middle of [`JobFrame::test_band`] — the routed outline's own waste band
/// where the job has one, so on an ordinary routed job the frame does not grow by a micron and
/// the cut is made in material that was going to be swarf. What is still not knowable here is
/// the blank itself: k2g models no stock or bed geometry, so the note names the size every time.
fn plan_test_cut(
    ctx: &AppCtx,
    raw: &StepRaw,
    cnc: &crate::data::model::profiles::MachineProfile,
    bit: &Tool,
    choice: &EngraveChoice,
    placement: &Placement,
    frame: &JobFrame,
) -> (Option<TestCut>, Vec<String>) {
    if !raw.engrave_copper.test_cut {
        return (None, Vec::new());
    }
    // The frame resolved the band; a step that asks for a test cut always has one, so `None`
    // here means the frame and this step disagree about the profile they read.
    let Some(band) = frame.test_band else {
        return (None, Vec::new());
    };
    if cnc.pause_tpl.trim().is_empty() {
        return (
            None,
            vec![format!(
                "'{}' has no pause primitive, so this program cannot stop for you to check an \
                 engraving test cut. None is planned: a witness groove the program runs \
                 straight past is not a test, and it would be cut in a corner nothing here \
                 knows is clear. Set the depth on a scrap board, or give the CNC profile a \
                 pause that really stops.",
                cnc.name,
            )],
        );
    }
    let rect = placement.board_rect_mm();
    let Some(path) = testcut::l_path(rect, band) else {
        // The band was reserved against the **job's** frame fixture and this step is placed
        // against its own, so a step engraving on a second fixture that zeroes on a different
        // corner finds the room on the wrong two edges. Reported rather than dropped in
        // silence: the operator ticked a box, and a program that quietly does not stop is the
        // failure this whole option exists to prevent.
        //
        // Also reaches here with no board at all, which is already reported elsewhere — hence
        // the guard, so a boardless job does not gain a second complaint about it.
        if ctx.board.is_none() {
            return (None, Vec::new());
        }
        return (
            None,
            vec![
                "No depth test cut is planned: the placement left no room for one. The job \
                 reserves that room against the fixture of its first step, so an engraving \
                 step set up on a fixture that zeroes on a different corner cannot have it — \
                 engrave in a step on the job's own fixture, or turn the test cut off."
                    .to_string(),
            ],
        );
    };

    let stop = VerifyStop {
        // Not the retract plane. The operator is about to put a hand and a loupe next to the
        // cut, so the tool goes to the height that clears the clamps and the fixture.
        z_clear: placement.z_safe(),
        advice: test_cut_advice(choice.width, choice.depth, bit.point_angle.as_degrees()),
        prompt: "Measure the test cut before the copper is engraved".to_string(),
    };

    // In the operator's own unit — this is a number they measure to, so a millimetre figure on
    // an imperial machine would be the one thing here they cannot act on. (The *emitted* text
    // above is the opposite case, and stays in mm for the reason given there.)
    //
    // Which band it landed in is the thing worth saying, because the two cases differ in what
    // they cost. Reusing the routed waste is free and the cut is in material the job removes
    // anyway; opening a band moves the board out, and therefore every coordinate in every
    // program of the job.
    let reuses_waste = frame.waste.is_some_and(|w| band.as_mm() <= w.as_mm() + 1e-9);
    let note = if reuses_waste {
        format!(
            "A depth test L, {} x {}, is cut down the middle of the band the outline routing \
             removes anyway — so it costs no extra blank and moves nothing.",
            fmt_len(ctx, Length::from_mm(rect.width())),
            fmt_len(ctx, Length::from_mm(rect.height())),
        )
    } else {
        format!(
            "A depth test L, {} x {}, is cut in a {} band just outside the board. This job does \
             not route its own outline, so that band is opened for the test cut and the work \
             origin moves out by it — re-zero before running, and check the blank reaches that \
             far.",
            fmt_len(ctx, Length::from_mm(rect.width())),
            fmt_len(ctx, Length::from_mm(rect.height())),
            fmt_len(ctx, band),
        )
    };

    (
        Some(TestCut {
            path,
            // Board top is Z0, so a depth is a negative machine Z. The *nominal* depth, not a
            // narrowed span's: what is being verified is the depth the pass was designed
            // around, and `choice.width` above is the width that depth produces.
            z_bottom: Length::from_mm(-choice.depth.as_mm()),
            stop,
        }),
        vec![note],
    )
}

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
    choice: &EngraveChoice,
    placement: &Placement,
) -> (Vec<EngraveSpan>, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();
    let Some(board) = ctx.board.as_ref() else {
        return (Vec::new(), warnings);
    };

    // A mill reaches the surface, so the face the step machines decides the layer. The
    // placement already mirrors a back-face step, and copper arrives in the same board
    // coordinates the holes do, so B.Cu needs nothing further.
    //
    // Both widths come from the **chosen bit**, not from the step's setting. The setting is
    // a minimum the operator states; what the pass has to lay out is the channel the bit in
    // the rack actually cuts, and how far that bit can be backed off where the board is
    // tight. Neither is knowable before a bit is picked.
    let spec = IsolationSpec {
        board_name: board.name.clone(),
        board_epoch: ctx.board_epoch,
        layer_id: if raw.machines_back { pcb::BACK_COPPER } else { pcb::FRONT_COPPER },
        width_nm: (choice.width.as_mm() * 1e6).round() as i64,
        // The narrowest cut this bit can make, at minimum penetration — **not** its tip.
        // The tip is a width it can never produce: a V-bit sunk to nothing cuts nothing,
        // and one that has only just cleared the copper is already wider than its tip. The
        // pass narrows down to here and reports whatever it still could not fit.
        min_width_nm: (choice.floor.as_mm() * 1e6).round() as i64,
        remove_islands: raw.engrave_copper.remove_islands,
    };

    let Some(isolation) = ctx.isolation.matching(&spec) else {
        // **Say so.** This used to return silently, on the argument that work in progress
        // is not a shortcoming of the job and that a note written now would still be
        // sitting there afterwards. The second half of that is simply false — the plan is
        // rebuilt every time the worker publishes, so the note clears itself — and the
        // first half cost an operator an afternoon: contours that never arrived produced a
        // step with no engrave block, which renders as a complete, green, empty job. The
        // copper was not engraved and nothing anywhere said why.
        if let Some(error) = ctx.isolation.error.as_ref() {
            warnings.push(format!("The copper could not be read: {error}"));
        } else {
            warnings.push(
                "The isolation contours for this face are still being computed, so the \
                 copper is not engraved in this program yet."
                    .to_string(),
            );
        }
        // What is being waited on, against what is held. If this ever sticks, the log names
        // the field that differs — which is the whole difference between a fault anyone can
        // fix and the one that took a board off the machine with its copper untouched.
        log_isolation_miss(&ctx.isolation, &spec);
        crate::runtime::isolation::request_isolation(spec);
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

    let place = |path: &[(i64, i64)]| -> Vec<Point> {
        path.iter()
            .map(|&(x, y)| {
                placement.xy(&pcb::BoardPoint {
                    x: Length::from_mm(x as f64 / 1e6),
                    y: Length::from_mm(y as f64 / 1e6),
                })
            })
            .collect()
    };

    let mut spans: Vec<EngraveSpan> = isolation
        .result
        .contours
        .iter()
        .enumerate()
        .map(|(n, contour)| EngraveSpan {
            source: format!("{}#{n}", contour.net),
            path: place(&contour.path),
            closed: contour.closed,
            // Negative machine-Z depth (board top is Z0; op-planner §6). Scaled from the
            // width this stretch actually achieved, not from the width that was asked for:
            // a narrowed stretch is a shallower cut, and that is the whole mechanism.
            z_bottom: Length::from_mm(-span_depth_mm(bit, contour.width_nm, choice.depth)),
        })
        .collect();

    // The island rings go in the SAME block, as ordinary engrave spans: same bit, same
    // face, same depth. So there is nothing to plan — the TSP orders them along with
    // everything else, the op table lists them and the 3D view draws them, all for free.
    //
    // At the nominal depth, not `span_depth_mm`: an island is cut at the width the bit was
    // chosen for, and only a contour that had to squeeze through a tight gap is shallower.
    spans.extend(isolation.clearing.paths.iter().enumerate().map(|(n, path)| EngraveSpan {
        source: format!("island #{}", n + 1),
        path: place(path),
        closed: true,
        z_bottom: Length::from_mm(-choice.depth.as_mm()),
    }));
    if let Some(note) = clearing_note(&isolation.clearing) {
        warnings.push(note);
    }

    (spans, warnings)
}

/// What to tell the operator about the copper the pass left standing.
///
/// One note for the step rather than one per island: an opportunist pass that narrates
/// every fragment of a board is a note nobody reads to the end.
///
/// The *left* half is the useful one. It says what a router would buy, the way
/// `IsolationResult::widest_workable_nm` says what changing the channel width would buy —
/// and until there is a router to offer, it is the only place a board's stranded copper is
/// mentioned at all.
fn clearing_note(clearing: &pcb::Clearing) -> Option<String> {
    if clearing.removed == 0 && clearing.left == 0 {
        return None;
    }
    let mm2 = |nm2: f64| nm2 / 1e12;

    let mut note = if clearing.removed == 0 {
        "No copper was left stranded that the engraver could take.".to_string()
    } else {
        format!(
            "Removed {} copper island{} ({:.2} mm²) the isolation pass left stranded.",
            clearing.removed,
            if clearing.removed == 1 { "" } else { "s" },
            mm2(clearing.removed_area_nm2),
        )
    };
    if clearing.left > 0 {
        note.push_str(&format!(
            " {} more {} left standing, the widest {:.2} mm across — too wide for the bit \
             already in the spindle to be worth it.",
            clearing.left,
            if clearing.left == 1 { "was" } else { "were" },
            clearing.widest_left_nm as f64 / 1e6,
        ));
    }
    // Never silently dropped: a piece of copper is cut, or it is accounted for. This is
    // copper inside an island the tool could not reach without touching a net — which
    // happens where the channel beside it had to narrow.
    if clearing.missed_area_nm2 > 0.0 {
        note.push_str(&format!(
            " {:.2} mm² of that was too close to a net for the bit to reach and is still \
             there.",
            mm2(clearing.missed_area_nm2),
        ));
    }
    Some(note)
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
///
/// The step's finishing allowance applies here too, and for the same reason it does on the
/// boundary — a wall is a wall. It comes back as a **second** map of spans, so the caller
/// can put every roughing pass in front of every finishing one. The allowance is per
/// cutout: the cutter was chosen to fit the opening as drawn, and an opening tight enough
/// to only just admit it has no room to stand one allowance further in.
#[allow(clippy::too_many_arguments)]
fn plan_cutout_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    routers: &RouterPlan,
    placement: &Placement,
    drill_targets: &mut Vec<DrillTarget>,
    slots: &std::collections::BTreeMap<String, u8>,
    setup: &assigner::Setup,
) -> Passes<SpansByRouter> {
    let mut rough_by_router: SpansByRouter = Default::default();
    let mut by_router: SpansByRouter = Default::default();
    let mut notes: Vec<String> = Vec::new();

    let Some(stitched) = ctx.stitched_board_data.as_ref() else {
        return Passes { rough: rough_by_router, finish: by_router, warnings: notes };
    };
    let cfg = &raw.route_cutouts;
    let placed_tabs = with_appdata(|data| data.job_edge_tabs());
    let mut corners_skipped = 0usize;
    // Island tabs asked to be perforated that the rack holds no drill for.
    let mut unperforated = 0usize;
    // Cutouts too tight to stand the cutter one allowance further in.
    let mut too_tight = 0usize;

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
        let Some(router) = ctx.tools.iter().find(|t| &t.id == router_id) else {
            continue;
        };

        let place = |path: &[(i64, i64)]| -> Vec<Point> {
            path.iter()
                .map(|&(x, y)| {
                    placement.xy(&pcb::BoardPoint {
                        x: Length::from_mm(x as f64 / 1e6),
                        y: Length::from_mm(y as f64 / 1e6),
                    })
                })
                .collect()
        };
        let points = place(&fit.wall_path);

        // The wall loop as the fit produced it, no orientation forced — the loop the
        // operator's stored tab nudges were measured along. See the same note on the
        // boundary: a nudge is a signed distance, so it has to be resolved to a point
        // before either pass turns the loop round.
        let Some(native) = outline::Loop::new(&points) else {
            continue;
        };

        // This opening's own cutter, not the edge kerf: a cutout's router is chosen by fit,
        // so what a tab here has to give up to leave its material is whatever fitted.
        let cutter_mm = router.diameter.as_mm();

        // A slug this step does not hold is a slug the roughing pass cuts loose, so there
        // is nothing left to finish against. An opening that leaves no slug at all — one
        // nowhere wider than twice the cutter — is cleared entirely by the wall pass and
        // has nothing to throw, so it takes its finishing pass like the boundary does.
        let held = cfg.retain_island || fit.slugs.is_empty();
        let (finishing_nm, note) = finishing_allowance(
            ctx.unit_system,
            raw.route_board.finishing,
            (router.diameter.as_mm() * 1e6).round() as i64,
            held,
            "interior openings",
        );
        // One reason, however many openings share it: the config that produced it is one
        // config, and a note per cutout would say the same sentence a dozen times.
        if let Some(note) = note {
            if !notes.contains(&note) {
                notes.push(note);
            }
        }

        // Eroding the wall path by the allowance is exactly `erode(C, R + f)` — erosion
        // composes — so the roughing loop costs one offset rather than a second `fit_cutout`
        // over the whole opening. Empty means the cutter cannot stand that far in at all;
        // more fragments than the wall path had means the allowance pinches the opening in
        // two, and cutting both lobes frees neither. Either way the opening is cut to size
        // in one pass rather than badly in two.
        let rough = (finishing_nm > 0)
            .then(|| pcb::offset_paths(&fit.wall_path, -(finishing_nm as f64)))
            .filter(|fragments| fragments.len() == 1)
            .and_then(|fragments| fragments.into_iter().next());
        if finishing_nm > 0 && rough.is_none() {
            too_tight += 1;
        }

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
                    is_pin: false,
                });
            }
        }

        // --- the slug, and the tab that holds it ---
        // Anchored as machine-space points, for the reason `tab_fractions` gives: the two
        // passes run different loops, and only a point means the same bridge on both.
        let mut tabs: Vec<(Point, f64, bool)> = Vec::new(); // (anchor, width, bites)
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
                let Some(tab) = outline::island_tab(&native, perimeter_mm, anchor, cfg.tab_ratio)
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
                    native.point_at(tab.at * native.length_mm() + nudge),
                    tab.width_mm,
                    tab.mouse_bites,
                ));
            }
        }

        // One width for the whole loop, since `cut_spans` cuts a single loop: the widest
        // of this cutout's tabs, so no island is held by less than it asked for.
        let width_mm = tabs.iter().map(|t| t.1).fold(0.0_f64, f64::max);
        let anchors: Vec<Point> = tabs.iter().map(|t| t.0).collect();

        // `retain_island` is a wish until a tab actually lands: `island_tab` declines a
        // slug the loop has no room to hold. A slug left with none is a slug the roughing
        // pass cuts loose, so the finishing pass goes with it — the same rule the boundary
        // applies to its own tabs, and knowable only here, per opening.
        let two_pass = finishing_nm > 0 && (fit.slugs.is_empty() || !anchors.is_empty());
        let rough = two_pass.then_some(rough).flatten();

        // The roughing pass: further **in** from the wall by the allowance, and cut
        // clockwise. The material being finished is the opening's wall, which lies outside
        // this loop, so a clockwise traveller keeps it on the left — conventional. It is
        // the mirror of the boundary's rule, and the same rule `gcode::routing` states for
        // every pocket.
        if let Some(rough) = rough {
            if let Some(path) = outline::Loop::new(&outline::oriented(place(&rough), false)) {
                let fractions = tab_fractions(&path, &anchors);
                for (n, span) in outline::cut_spans(&path, &fractions, width_mm, cutter_mm)
                    .into_iter()
                    .enumerate()
                {
                    rough_by_router
                        .entry(router_id.clone())
                        .or_default()
                        .push(OutlineSpan {
                            source: format!("{label}#{kind_index}.span{n}{}", PASS_SUFFIX[0]),
                            path: span,
                        });
                }
            }
        }

        // The pass that makes the wall: counter-clockwise, so the wall is on the cutter's
        // right and the cut is climb.
        let Some(path) = outline::Loop::new(&outline::oriented(points, true)) else {
            continue;
        };
        let fractions = tab_fractions(&path, &anchors);
        let suffix = if two_pass { PASS_SUFFIX[1] } else { "" };

        for (n, span) in outline::cut_spans(&path, &fractions, width_mm, cutter_mm)
            .into_iter()
            .enumerate()
        {
            by_router
                .entry(router_id.clone())
                .or_default()
                .push(OutlineSpan {
                    source: format!("{label}#{kind_index}.span{n}{suffix}"),
                    path: span,
                });
        }

        for (n, (_, _, bites)) in tabs.iter().enumerate() {
            if !bites {
                continue;
            }
            let Some(bite_tool) = mouse_bite_drill(ctx, slots) else {
                unperforated += 1;
                continue;
            };
            // On the final wall loop and once, as on the boundary.
            for (h, centre) in
                outline::mouse_bite_centres(&path, fractions[n], width_mm, bite_tool.1)
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
                    is_pin: false,
                });
            }
        }
    }

    if unperforated > 0 {
        notes.push(format!(
            "{unperforated} island tab(s) are left solid: they are wide enough to want mouse \
             bites, but this step loads no drill to perforate them. Add a small drill to the \
             toolset, or expect to cut these tabs rather than snap them."
        ));
    }
    if corners_skipped > 0 {
        notes.push(format!(
            "{corners_skipped} sharp corner(s) were left as the cutter rounds them: no drill \
             in the rack falls in the size band that would relieve them without leaving an \
             uncut web. Add a smaller drill to the toolset."
        ));
    }
    if too_tight > 0 {
        notes.push(format!(
            "{too_tight} interior opening(s) are cut to size in one pass: they have no room \
             for the finishing allowance as well as the cutter that fits them. The opening \
             is still cut to its drawn size — only the finishing pass is missing."
        ));
    }
    Passes { rough: rough_by_router, finish: by_router, warnings: notes }
}

fn plan_outline_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    routers: &RouterPlan,
    placement: &Placement,
    drill_targets: &mut Vec<DrillTarget>,
    slots: &std::collections::BTreeMap<String, u8>,
) -> Result<Passes<Vec<OutlineSpan>>, String> {
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

    // How much wall this step leaves for the finishing pass, and the offsets that rough it
    // out. The roughing loop is simply the same offset taken one allowance further onto
    // the waste side — `routing_offset` already grows the boundary, so a larger radius is
    // the whole of the change and the `pcb` crate needs nothing new.
    //
    // Note what this does to the channel: it ends up `kerf + finishing` wide rather than
    // `kerf`. The board still comes out at its drawn size — the finishing pass puts the
    // final wall exactly where the single pass used to — and all the extra width is on the
    // waste side.
    let (finishing_nm, finishing_note) = finishing_allowance(
        ctx.unit_system,
        edge.finishing,
        radius_nm * 2,
        edge.outline.tabs,
        "board outline",
    );
    let rough_offsets = (finishing_nm > 0)
        .then(|| pcb::routing_offset(&stitched.contours, radius_nm + finishing_nm));

    let mut rough_spans: Vec<OutlineSpan> = Vec::new();
    let mut spans: Vec<OutlineSpan> = Vec::new();
    let mut vanished = 0usize;
    // Tabs the outline had no room for, at the clearance the distribution keeps.
    let mut crowded = 0usize;
    // Interior openings this step leaves in the board, because nothing on it cuts them.
    let mut uncut_openings = 0usize;
    // Tabs asked to be perforated that the rack holds no drill for.
    let mut unperforated = 0usize;
    // Tabs whose bridge is too narrow to carry a hole at the drill's own clearances, and
    // the drill that could not be fitted into them — kept for the note, which is only
    // useful if it names the bridge a bite would have needed.
    let mut too_narrow = 0usize;
    let mut unbitten = Length::from_mm(0.0);
    // Contours that wanted a finishing pass but ended up with no tab to hold them.
    let mut unheld = 0usize;
    // Cutouts are numbered among themselves, as `job.yaml#/edge_tabs/index` means it.
    let (mut outer_n, mut cutout_n) = (0usize, 0usize);

    for (contour_n, (contour, offset)) in stitched.contours.iter().zip(offsets).enumerate() {
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
        let place = |path: &[(i64, i64)]| -> Vec<Point> {
            path.iter()
                .map(|&(x, y)| {
                    placement.xy(&pcb::BoardPoint {
                        x: Length::from_mm(x as f64 / 1e6),
                        y: Length::from_mm(y as f64 / 1e6),
                    })
                })
                .collect()
        };
        let points = place(&offset);

        // The loop as the offset library hands it back — no orientation forced on it.
        //
        // It exists only to resolve the operator's stored tab nudges, and that is exactly
        // why it is left alone: a nudge is a *signed distance along the loop*, so its
        // meaning flips the moment the loop is turned round. Resolving nudges here, on the
        // one loop whose winding no longer depends on anything this change does, is what
        // lets every tab already positioned on every existing job keep the position it was
        // given. What comes out is a point, and a point means the same thing whichever way
        // either pass runs.
        let Some(native) = outline::Loop::new(&points) else {
            continue;
        };

        let retention = edge.outline;
        // The material a tab leaves standing, and the cutter that decides what the spans
        // either side of it have to give up to leave that much. The two travel together
        // from here on: a tab width without a kerf is a tab width that means nothing.
        let width_mm = retention.width.as_mm();
        let kerf_mm = router.diameter.as_mm();

        // Where the tabs go. Distribution runs on the contour's own **straight
        // segments**, not on the offset polyline — the offset flattens every rounded
        // corner into dozens of chords, so "segments" there would be meaningless. Each
        // computed anchor is then placed and projected onto the offset path, which for
        // an outward offset of a straight run is exactly the perpendicular foot.
        let anchors: Vec<Point> = if retention.tabs {
            let found =
                outline::distribute_tabs(
                    &straight_segments_mm(contour),
                    retention.count,
                    width_mm,
                    kerf_mm,
                );
            if found.len() < retention.count {
                crowded += retention.count - found.len();
            }
            found
                .iter()
                .enumerate()
                .map(|(n, anchor)| {
                    let at = native.nearest_fraction(placement.xy(&pcb::BoardPoint {
                        x: Length::from_mm(anchor.point.0),
                        y: Length::from_mm(anchor.point.1),
                    }));
                    // The operator's own nudge, along the loop from the computed anchor.
                    let nudge = placed_tabs
                        .iter()
                        .find(|t| t.contour == kind && t.index == kind_index && t.tab == n)
                        .map(|t| t.offset.as_mm())
                        .unwrap_or(0.0);
                    native.point_at(at * native.length_mm() + nudge)
                })
                .collect()
        } else {
            Vec::new()
        };

        // Retention is a mode until a tab actually lands. `count: 0`, or sides with no
        // room for the ones asked for, both leave this contour with none — and a loop cut
        // with no tabs is a loop cut free, whatever the mode says. Checked here rather
        // than with the mode, because it is only knowable per contour.
        let two_pass = finishing_nm > 0 && !anchors.is_empty();
        if finishing_nm > 0 && anchors.is_empty() {
            unheld += 1;
        }

        // The roughing pass, when there is one: further out by the allowance, and cut
        // **conventional**. The finishing pass below runs the other way round the same
        // wall, which is what makes it climb.
        //
        // The board is inside this loop, so material lies to the left of a
        // counter-clockwise traveller — conventional. (The rule, and the handedness
        // argument it rests on, is in `gcode::routing`.)
        if let Some(rough) = two_pass
            .then_some(rough_offsets.as_ref())
            .flatten()
            .and_then(|offsets| offsets.get(contour_n))
            .and_then(|offset| offset.as_ref())
        {
            if let Some(path) = outline::Loop::new(&outline::oriented(place(rough), true)) {
                let tabs = tab_fractions(&path, &anchors);
                for (n, span) in outline::cut_spans(&path, &tabs, width_mm, kerf_mm)
                    .into_iter()
                    .enumerate()
                {
                    rough_spans.push(OutlineSpan {
                        source: format!("{label}#{kind_index}.span{n}{}", PASS_SUFFIX[0]),
                        path: span,
                    });
                }
            }
        }

        // The pass that makes the edge: clockwise round the boundary, so the board is to
        // the cutter's right and the cut is climb. When there is no roughing pass this is
        // simply the whole cut, taken to size in one go — the only thing the allowance
        // changed is that there is now something in front of it.
        let Some(path) = outline::Loop::new(&outline::oriented(points, false)) else {
            continue;
        };
        let tabs = tab_fractions(&path, &anchors);
        let suffix = if two_pass { PASS_SUFFIX[1] } else { "" };

        for (n, span) in outline::cut_spans(&path, &tabs, width_mm, kerf_mm)
            .into_iter()
            .enumerate()
        {
            spans.push(OutlineSpan {
                source: format!("{label}#{kind_index}.span{n}{suffix}"),
                path: span,
            });
        }

        // Mouse bites are drills, so they join the drill phase rather than the route one.
        if retention.mouse_bites {
            // Counted, not silently dropped: a tab wide enough to ask for bites is a tab
            // wide enough to tear the board when it is snapped solid.
            match mouse_bite_drill(ctx, slots) {
                None => unperforated += tabs.len(),
                Some(bite_tool) => {
                    for (n, tab) in tabs.iter().enumerate() {
                        // Placed on the **final** wall loop, and once, however many passes
                        // cut it. The bridge they perforate spans the whole channel, which
                        // an allowance widens to `kerf + finishing`, so a hole centred on
                        // this loop sits half an allowance off the bridge's middle — 50 µm
                        // at the default, inside a bridge millimetres across. Chasing that
                        // would cost a third offset of every contour to move a hole by less
                        // than a drill's runout.
                        let centres =
                            outline::mouse_bite_centres(&path, *tab, width_mm, bite_tool.1);
                        // A bridge too narrow to carry a hole at its clearances is a
                        // different fault from a rack with no drill in it, and wants a
                        // different sentence: this one is fixed by widening the tab or
                        // fitting a finer drill, neither of which the other note suggests.
                        if centres.is_empty() {
                            // The widest drill that failed, so the note quotes the bridge
                            // that would actually have been needed rather than an easier
                            // number from some other tab.
                            if bite_tool.1.as_mm() > unbitten.as_mm() {
                                unbitten = bite_tool.1;
                            }
                            too_narrow += 1;
                        }
                        for (h, centre) in centres.into_iter().enumerate() {
                            drill_targets.push(DrillTarget {
                                source: format!("{label}#{kind_index}.bite{n}.{h}"),
                                // The span geometry is already placed, so unplace it: the
                                // drill planner places its own targets.
                                at: placement.unplace(&centre),
                                tool_id: bite_tool.0.clone(),
                                diameter: bite_tool.1,
                                z_bottom: bite_tool.2,
                                // One run, so the perforation is drilled in order along
                                // the tab rather than being scattered through the tour.
                                chain: Some(format!("{label}#{kind_index}.bite{n}")),
                                is_pin: false,
                            });
                        }
                    }
                }
            }
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    warnings.extend(finishing_note);
    if uncut_openings > 0 {
        warnings.push(format!(
            "{uncut_openings} interior opening(s) are left in the board: the edge pass cuts \
             the boundary only. Add 'Route interior cutouts' to this step to cut them, each \
             with a cutter chosen to fit."
        ));
    }
    if unperforated > 0 {
        warnings.push(format!(
            "{unperforated} retaining tab(s) are left solid: they are wide enough to want mouse \
             bites, but this step loads no drill to perforate them. Add a small drill to the \
             toolset, or expect to cut these tabs rather than snap them."
        ));
    }
    if too_narrow > 0 {
        warnings.push(format!(
            "{too_narrow} retaining tab(s) are left solid: a mouse bite needs {} of tab to hold \
             one {} hole with a drill's width of board either side of it, and these are narrower \
             than that. Widen the tabs, fit a finer drill, or expect to cut them rather than \
             snap them.",
            fmt_len(ctx, Length::from_mm(outline::mouse_bite_span_mm(1, unbitten.as_mm()))),
            fmt_len(ctx, unbitten),
        ));
    }
    if crowded > 0 {
        warnings.push(format!(
            "{crowded} retaining tab(s) could not be placed: the outline's straight sides \
             have no room left at the required clearance. Widen the board's sides, narrow \
             the tabs, or ask for fewer."
        ));
    }
    if unheld > 0 {
        warnings.push(format!(
            "{unheld} outline contour(s) are cut to size in one pass: retention is set to \
             tabs, but none was placed on them, so the roughing pass would already have cut \
             them free. Ask for at least one tab, or set the finishing allowance to 0."
        ));
    }
    if vanished > 0 {
        return Err(format!(
            "{vanished} outline contour(s) are smaller than the {} router and vanish under \
             its kerf, so they cannot be cut. Stock a smaller cutter.",
            router.name
        ));
    }
    Ok(Passes { rough: rough_spans, finish: spans, warnings })
}

/// Cut spans grouped by the router that cuts them — a cutout's cutter is chosen by fit,
/// so two openings on one board may want two.
type SpansByRouter = std::collections::BTreeMap<String, Vec<OutlineSpan>>;

/// What one contour-planning call produced: the two passes, and what the operator should
/// know about them.
///
/// Named rather than returned as a tuple because the two fields are the same type and the
/// order between them is the whole point — a caller that puts them the wrong way round
/// finishes every wall before roughing it, which no type would catch and no test of the
/// geometry would either. `rough` is empty whenever the step leaves no allowance, which is
/// the common case and not a shortcoming.
struct Passes<T> {
    rough: T,
    finish: T,
    warnings: Vec<String>,
}

/// What each pass adds to a span's feature id, in cut order.
///
/// Empty for a single-pass cut: a step with no finishing allowance keeps exactly the
/// feature ids it had before this existed, so nothing that reads them — the Machining
/// view's op table, a diagnostic, an operator comparing two programs — has to learn a new
/// spelling for a job that did not change.
const PASS_SUFFIX: [&str; 2] = [".rough", ".finish"];

/// The finishing allowance this step can actually honour, in nanometres, with the reason
/// when it cannot honour the one that was asked for.
///
/// Zero is not a failure — it is the single-pass cut, and the answer for the great many
/// steps that leave `finishing` at nothing. A reason comes back only when an allowance
/// *was* asked for and had to be dropped, because that is the case the operator set a
/// value for and would otherwise never learn was ignored.
///
/// `kerf_nm` is the cutter's full diameter. The two passes sweep `f .. kerf+f` and
/// `0 .. kerf`, so they overlap only while `f < kerf`; at or beyond it the roughing pass
/// and the finishing pass would leave a ring of material between them and the piece would
/// not come free at all.
fn finishing_allowance(
    units: units::UserUnitSystem,
    finishing: Length,
    kerf_nm: i64,
    retained: bool,
    what: &str,
) -> (i64, Option<String>) {
    let asked_nm = (finishing.as_mm() * 1e6).round() as i64;
    if asked_nm <= 0 {
        return (0, None);
    }
    if !retained {
        return (
            0,
            Some(format!(
                "The finishing pass on the {what} was dropped and it is cut to size in one \
                 pass: the roughing pass would already have cut it free, and a finishing \
                 pass has to cut a wall that is still held. Retain it with tabs, or set the \
                 finishing allowance to 0."
            )),
        );
    }
    if asked_nm >= kerf_nm {
        return (
            0,
            Some(format!(
                "The finishing allowance ({}) is not smaller than the cutter that would \
                 remove it, so the two passes would not overlap and a ring of material \
                 would be left uncut. The {what} is cut to size in one pass. Reduce the \
                 allowance well below the kerf.",
                finishing.unit_display(units).user,
            )),
        );
    }
    (asked_nm, None)
}

/// Where this pass's tabs sit on *its own* loop, as fractions of it.
///
/// The tabs arrive as machine-space **points** rather than as fractions, and this is why:
/// the roughing loop and the finishing loop are different lengths (the roughing one runs
/// one allowance further out), and they may run opposite ways round, so the same fraction
/// on the two is not the same place on the board. A tab has to be the same physical bridge
/// on both passes or the finishing pass cuts through what the roughing pass left holding
/// the piece. Projecting the point onto each loop is what makes it so.
fn tab_fractions(path: &outline::Loop, anchors: &[Point]) -> Vec<f64> {
    anchors.iter().map(|&anchor| path.nearest_fraction(anchor)).collect()
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
/// which leaves the tab solid rather than inventing a tool (the callers say so).
///
/// `Drillbit` specifically, and not merely "not a router" as this once tested — the same
/// trap [`pick_pin_tool`] documents. [`ToolKind::from_kind_label`] falls through to
/// `Endmill` for anything it does not recognise, so "not a router" accepted a V-bit, an
/// engraver or a typo'd kind string, and the smallest-first rule then *preferred* them: a
/// V-bit's diameter is its **tip width**, so a ⌀0.20 V60 sorts below every real drill and
/// wins every time. Sinking one through the board would not drill a 0.20 hole at all but a
/// cone opening out to ⌀1.9 at the surface of a 1.6 mm board — it would eat the tab it was
/// meant to perforate, and blunt the isolation bit doing it.
fn mouse_bite_drill(
    ctx: &AppCtx,
    slots: &std::collections::BTreeMap<String, u8>,
) -> Option<(String, Length, Length)> {
    let setup = build_setup(ctx, None);
    let tool = pick_mouse_bite_drill(&ctx.tools, slots)?;
    Some((
        tool.id.clone(),
        tool.diameter,
        assigner::router_plunge(&setup),
    ))
}

/// The selection rule alone, over the tools and the rack — split out from
/// [`mouse_bite_drill`] so it can be exercised without a whole [`AppCtx`].
fn pick_mouse_bite_drill<'a>(
    tools: &'a [Tool],
    slots: &std::collections::BTreeMap<String, u8>,
) -> Option<&'a Tool> {
    tools
        .iter()
        .filter(|t| {
            slots.contains_key(&t.id)
                && matches!(ToolKind::from_kind_label(&t.kind), ToolKind::Drillbit)
        })
        .min_by_key(|t| t.diameter.as_um().round() as i64)
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
mod finishing_tests {
    use super::*;

    /// A 2 mm cutter, the schema's default kerf, in nanometres.
    const KERF_NM: i64 = 2_000_000;

    fn allowance(mm: f64, retained: bool) -> (i64, Option<String>) {
        finishing_allowance(
            units::UserUnitSystem::Metric,
            Length::from_mm(mm),
            KERF_NM,
            retained,
            "board outline",
        )
    }

    /// The ordinary case: the allowance asked for is the allowance left on the wall.
    #[test]
    fn a_workable_allowance_is_honoured_without_comment() {
        let (nm, note) = allowance(0.1, true);
        assert_eq!(nm, 100_000);
        assert!(note.is_none(), "nothing went wrong, so there is nothing to say");
    }

    /// Zero is the single-pass cut, not a failure — and by far the commonest answer. It
    /// must not produce a note, or every job that leaves the field alone grows one.
    #[test]
    fn no_allowance_is_silent() {
        assert_eq!(allowance(0.0, true), (0, None));
        assert_eq!(allowance(0.0, false), (0, None), "not even with nothing holding it");
    }

    /// The rule the operator asked for: a finishing pass cuts a wall, and a wall the
    /// roughing pass has already cut free is not a wall any more.
    #[test]
    fn an_unheld_piece_loses_its_finishing_pass_and_is_told_so() {
        let (nm, note) = allowance(0.1, false);
        assert_eq!(nm, 0, "cut to size in one pass instead");
        assert!(note.expect("the operator set a value and must learn it was dropped").contains("tabs"));
    }

    /// The two passes sweep `f..kerf+f` and `0..kerf`, so at `f == kerf` they stop
    /// overlapping and a ring of material survives between them — the board would not come
    /// free at all. Refused at the boundary, not just past it.
    #[test]
    fn an_allowance_the_cutter_cannot_span_is_refused() {
        assert_eq!(allowance(2.0, true).0, 0, "exactly the kerf already fails to overlap");
        assert_eq!(allowance(3.0, true).0, 0, "and wider is worse");
        assert!(allowance(2.0, true).1.expect("said out loud").contains("overlap"));

        assert_eq!(allowance(1.999, true).0, 1_999_000, "just inside still overlaps");
    }

    /// Retention is checked before the kerf: a piece nothing holds cannot be finished
    /// whatever the allowance, and being told about the kerf instead would send the
    /// operator to fix the wrong field.
    #[test]
    fn the_holding_rule_is_reported_ahead_of_the_kerf_rule() {
        let note = allowance(5.0, false).1.expect("both rules are broken");
        assert!(note.contains("tabs"), "the one that has to be fixed first: {note}");
    }
}

#[cfg(test)]
mod mouse_bite_tests {
    use super::*;

    /// A stock tool of the given kind and diameter; nothing else is read here.
    fn tool(id: &str, kind: &str, diameter_mm: f64) -> Tool {
        Tool {
            id: id.to_string(),
            composite_name: format!("{kind} {diameter_mm}mm"),
            name: format!("{kind} {diameter_mm}mm"),
            kind: kind.to_string(),
            diameter: Length::from_mm(diameter_mm),
            catalog_diameter: None,
            point_angle: units::Angle::from_degrees(118.0),
            catalog_point_angle: None,
            flute_length: Some(Length::from_mm(30.0)),
            z_min_depth: None,
            table_feed: None,
            catalog_table_feed: None,
            z_feed: None,
            catalog_z_feed: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: crate::data::model::ToolStatus::InStock,
            preference: crate::data::model::ToolPreference::Neutral,
            source_catalog: "Test".to_string(),
            manufacturer: None,
            sku: None,
        }
    }

    /// Every tool racked, which is the case the rule is about.
    fn racked(tools: &[Tool]) -> std::collections::BTreeMap<String, u8> {
        tools
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i as u8 + 1))
            .collect()
    }

    /// The reported bug. An isolation step racks a V-bit whose ⌀ is its *tip width*, so
    /// smallest-first put a 0.20 V60 ahead of every drill on the machine — and the plan
    /// then sank a cone through the tab it was supposed to perforate.
    #[test]
    fn a_v_bit_never_drills_a_mouse_bite() {
        let tools = vec![
            tool("vbit", "V-Bit", 0.2),
            tool("drill", "Drill", 1.0),
            tool("router", "Router", 2.0),
        ];
        let chosen = pick_mouse_bite_drill(&tools, &racked(&tools));

        assert_eq!(
            chosen.map(|t| t.id.as_str()),
            Some("drill"),
            "the 0.2 V-bit is the smallest tool in the rack, and still must not be picked"
        );
    }

    /// The other two kinds that reach a rack are ruled out by the same test, and for the
    /// same reason: only a drill is a bit whose diameter is the hole it makes.
    #[test]
    fn nor_does_an_engraver_or_an_end_mill() {
        let tools = vec![
            tool("engraver", "Engraver", 0.1),
            tool("endmill", "End Mill", 0.8),
            tool("drill", "Drill", 1.2),
        ];
        let chosen = pick_mouse_bite_drill(&tools, &racked(&tools));

        assert_eq!(chosen.map(|t| t.id.as_str()), Some("drill"));
    }

    /// The rule proper, once the field is drills: the smallest hole that still breaks.
    #[test]
    fn the_smallest_racked_drill_wins() {
        let tools = vec![
            tool("big", "Drill", 1.0),
            tool("small", "Drill", 0.5),
            tool("unracked", "Drill", 0.3),
        ];
        let mut slots = racked(&tools);
        slots.remove("unracked");

        let chosen = pick_mouse_bite_drill(&tools, &slots);

        assert_eq!(
            chosen.map(|t| t.id.as_str()),
            Some("small"),
            "a finer drill off the rack is not worth a tool change the step never planned"
        );
    }

    /// No drill loaded leaves the tab solid — the callers count that and say so, rather
    /// than reaching for whatever else is in the rack.
    #[test]
    fn a_rack_without_a_drill_perforates_nothing() {
        let tools = vec![tool("vbit", "V-Bit", 0.2), tool("router", "Router", 2.0)];
        assert!(pick_mouse_bite_drill(&tools, &racked(&tools)).is_none());
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

/// The diagnostics that stand between a step that could not engrave and a program that
/// looks finished without it.
///
/// These are the guards for the fault an operator hit on the bench: contours that never
/// arrived produced a step with no engrave block, which rendered as a complete, green,
/// empty job with nothing anywhere saying the copper had not been touched.
#[cfg(test)]
mod engrave_diagnostic_tests {
    use super::*;
    use crate::runtime::isolation::{Isolation, IsolationSpec};

    fn spec(width_nm: i64, epoch: u64) -> IsolationSpec {
        IsolationSpec {
            board_name: "demo".into(),
            board_epoch: epoch,
            layer_id: pcb::FRONT_COPPER,
            width_nm,
            min_width_nm: 150_000,
            remove_islands: true,
        }
    }

    fn state_holding(held: Option<IsolationSpec>) -> crate::runtime::isolation::IsolationState {
        let mut state = crate::runtime::isolation::IsolationState::default();
        if let Some(held) = held {
            state.ready.insert(
                held.layer_id,
                std::sync::Arc::new(Isolation {
                    spec: held,
                    result: Default::default(),
                    copper_warnings: Vec::new(),
                    copper_layer_count: 2,
                    clearing: Default::default(),
                }),
            );
        }
        state
    }

    /// **The step says what it took and what it left.** The left half is the useful one:
    /// until there is a router to offer, this note is the only place a board's stranded
    /// copper is mentioned at all, and an operator who cannot see it has no way to know
    /// there is copper on their board that the design never drew.
    #[test]
    fn the_clearing_note_says_what_was_taken_and_what_was_left() {
        assert_eq!(clearing_note(&pcb::Clearing::default()), None, "a clean board says nothing");

        let took = pcb::Clearing {
            removed: 14,
            removed_area_nm2: 0.42e12,
            ..Default::default()
        };
        let note = clearing_note(&took).expect("a note");
        assert!(note.contains("14 copper islands"), "{note}");
        assert!(note.contains("0.42 mm²"), "{note}");
        assert!(!note.contains("left standing"), "nothing was left: {note}");

        let left = pcb::Clearing { left: 3, widest_left_nm: 910_000, ..took.clone() };
        let note = clearing_note(&left).expect("a note");
        assert!(note.contains("3 more were left standing"), "{note}");
        assert!(note.contains("0.91 mm across"), "the number a router would be judged on: {note}");

        // Singulars, because "1 islands" and "1 more were left" read as a bug in the tool.
        let one = pcb::Clearing {
            removed: 1,
            removed_area_nm2: 0.01e12,
            left: 1,
            widest_left_nm: 800_000,
            ..Default::default()
        };
        let note = clearing_note(&one).expect("a note");
        assert!(note.contains("1 copper island ("), "{note}");
        assert!(note.contains("1 more was left standing"), "{note}");

        // Copper the bit could not reach without touching a net is named, never dropped.
        let missed = pcb::Clearing { missed_area_nm2: 0.05e12, ..took.clone() };
        let note = clearing_note(&missed).expect("a note");
        assert!(note.contains("0.05 mm²"), "{note}");
        assert!(note.contains("too close to a net"), "{note}");

        // And a board where every island was too wide still gets told so.
        let none = pcb::Clearing { left: 2, widest_left_nm: 1_200_000, ..Default::default() };
        let note = clearing_note(&none).expect("a note");
        assert!(note.starts_with("No copper was left stranded"), "{note}");
        assert!(note.contains("2 more were left standing"), "{note}");
    }

    /// **The miss names the field that differs.** The spec is six fields and any one of
    /// them can hold an answer back; from the outside every case looks the same — no
    /// engraving, and a plan that keeps asking. This is what makes a persistent miss
    /// diagnosable from a log the operator can hand over.
    ///
    /// Asserted through the same comparison `matching` makes, so the two cannot drift into
    /// disagreeing about what counts as the same question.
    #[test]
    fn a_spec_that_differs_in_one_field_does_not_match() {
        let held = spec(254_000, 1);
        let state = state_holding(Some(held.clone()));

        assert!(state.matching(&held).is_some(), "the identical question matches");

        for (label, other) in [
            ("width", IsolationSpec { width_nm: 172_000, ..held.clone() }),
            ("floor", IsolationSpec { min_width_nm: 109_000, ..held.clone() }),
            ("epoch", IsolationSpec { board_epoch: 2, ..held.clone() }),
            ("board", IsolationSpec { board_name: "other".into(), ..held.clone() }),
            ("layer", IsolationSpec { layer_id: pcb::BACK_COPPER, ..held.clone() }),
            // The one that is a *setting* rather than a measurement, and so the one an
            // operator changes while the board sits still. Left out of the spec, ticking
            // "Remove islands" would match the held answer and do nothing whatever until
            // the board was reloaded.
            ("islands", IsolationSpec { remove_islands: false, ..held.clone() }),
        ] {
            assert!(
                state.matching(&other).is_none(),
                "a spec differing only in its {label} must not match",
            );
            // And it must not panic on the way to saying so, whichever field it is.
            log_isolation_miss(&state, &other);
        }
    }

    /// Nothing held at all is the ordinary first plan after a board loads, and it has to be
    /// reportable too — not just the case where a stale answer is present.
    #[test]
    fn a_miss_with_nothing_held_is_still_reported() {
        log_isolation_miss(&state_holding(None), &spec(254_000, 1));
    }

    /// **The note that broke the silence.** `plan_engrave_spans` returning nothing used to
    /// push no note at all, on the argument that work in progress is not a shortcoming. The
    /// plan is rebuilt on every publish, so the note clears itself — and without it the
    /// step is indistinguishable from one that had no copper to cut.
    #[test]
    fn a_step_waiting_on_contours_says_so_rather_than_returning_quietly() {
        // Asserted on the source rather than by driving a plan, which needs a live board,
        // a datastore and a KiCad connection. What is being guarded is that the early
        // return is not silent, and that is a property of the text.
        let source = include_str!("machining_plan.rs");
        // Anchored on the not-ready block itself, not on the function: `plan_engrave_spans`
        // returns `(Vec::new(), warnings)` from its no-board guard first, and scanning to
        // the first of those measures the wrong early return entirely.
        let body = source
            .split_once("ctx.isolation.matching(&spec) else {")
            .expect("the not-ready path is a let-else on `matching`")
            .1;
        let early_return = body
            .find("return (Vec::new(), warnings);")
            .expect("the not-ready path returns early");
        let head = &body[..early_return];

        assert!(
            head.contains("still being computed"),
            "the not-ready path must push a note before returning, or a step whose \
             contours never arrive renders as a complete, empty job",
        );
        assert!(
            head.contains("log_isolation_miss"),
            "and must log which field differs, or a persistent miss is undiagnosable",
        );
    }

    /// **A step that engraves and produces no engraving is a fault.** The one invariant
    /// nothing checked, and the reason a program came off the planner drilled, routed and
    /// with its copper untouched while every view showed it complete.
    #[test]
    fn a_step_that_engraves_nothing_is_flagged() {
        let source = include_str!("machining_plan.rs");
        let body = source
            .split_once("let engraved = plan_engrave(")
            .expect("the engrave block is built here")
            .1;

        let check = body.find("engraved.is_none()").expect("the empty case is checked");
        let pushed = body.find("Do not run it as it is").expect("and says what it means");
        assert!(check < pushed, "the check has to be what raises the note");
        assert!(
            check < body.find("blocks.extend(engraved)").expect("the block is added"),
            "checked before it is folded into the plan, while it can still be told apart",
        );
    }

    /// **The operator measures a width and turns a Z dial, so the stop has to convert.**
    ///
    /// This is the one piece of arithmetic in the feature that a person acts on directly, with
    /// the manual shut and the spindle stopped. Getting it inverted, or quoting it to a
    /// precision coarser than the thing being measured, sends them the wrong way by a
    /// believable-looking amount.
    #[test]
    fn the_stop_converts_the_width_the_operator_measures_into_the_z_they_adjust() {
        // A 0.1 mm tip 60-degree V-bit at one ounce of copper plus minimum penetration.
        let advice = super::test_cut_advice(
            Length::from_mm(0.1635),
            Length::from_mm(0.055),
            60.0,
        );
        let text = advice.join("\n");

        assert!(
            text.contains("0.164 mm across, 0.055 mm deep"),
            "the figures must resolve tens of microns, not hundredths:\n{text}",
        );
        // 1 / (2*tan(30)) = 0.8660.
        assert!(
            text.contains("Width error x 0.87 = the Z correction: 0.10 mm out is 0.087 mm of Z"),
            "the width-to-Z conversion is the number they act on:\n{text}",
        );
        assert!(text.contains("Too narrow"), "and both directions are named:\n{text}");
        assert!(text.contains("Too wide"), "and both directions are named:\n{text}");
        assert!(
            text.contains("shift the work offset in X and Y"),
            "going shallower needs fresh material, which is the half that is easy to omit:\n{text}",
        );

        // A finer cone turns the same width error into much more Z — the case where a wrong
        // conversion, or none, costs a board.
        let fine = super::test_cut_advice(Length::from_mm(0.2), Length::from_mm(0.05), 30.0)
            .join("\n");
        assert!(fine.contains("0.10 mm out is 0.187 mm of Z"), "{fine}");
    }

    /// **A flat tip gets no conversion, because there is not one.** Its width does not move
    /// with depth, so a multiplier here would be a number invented to fill the sentence — and
    /// an operator who trusted it would dial Z from a reading that says nothing about Z.
    #[test]
    fn a_tool_whose_width_does_not_follow_its_depth_is_not_given_a_conversion() {
        let text = super::test_cut_advice(Length::from_mm(0.2), Length::from_mm(0.05), 180.0)
            .join("\n");
        assert!(!text.contains("Z correction"), "no conversion is offered:\n{text}");
        assert!(text.contains("one width at any depth"), "and it says why:\n{text}");
    }

    /// **Nothing said at the stop may close a G-code comment.** The advice goes out through the
    /// machine's `comment` primitive, which every bundled profile renders as `( {text} )`; a
    /// bracket inside the text ends the comment early and feeds the rest of the sentence to the
    /// parser as motion.
    #[test]
    fn the_operator_text_carries_nothing_that_would_close_a_gcode_comment() {
        for angle in [30.0, 60.0, 90.0, 180.0] {
            for line in super::test_cut_advice(Length::from_mm(0.2), Length::from_mm(0.05), angle)
            {
                assert!(
                    !line.contains(['(', ')', '%', ';', '\n']),
                    "unsafe for a comment line: {line:?}",
                );
                assert!(line.is_ascii(), "and must be plain ASCII: {line:?}");
            }
        }
    }

    /// **The depth test cut is decided before the block that carries it is built**, and from
    /// the nominal depth rather than a span's.
    ///
    /// Both halves are one-line mistakes with no symptom. Built after the call, the test cut
    /// silently never reaches the program. Taken from `span_depth_mm` instead of
    /// `choice.depth`, the operator measures a channel narrowed for one tight stretch of the
    /// board and then sets the machine's Z from it — which is the wrong depth for the whole
    /// rest of the pass.
    #[test]
    fn the_test_cut_is_decided_before_the_block_and_from_the_nominal_depth() {
        let source = include_str!("machining_plan.rs");
        let built = source
            .find("let (test_cut, test_notes) =")
            .expect("the test cut is built in plan_step");
        let used = source
            .find("let engraved = plan_engrave(")
            .expect("the engrave block is built here");
        assert!(built < used, "the test cut has to exist before the block that carries it");

        let body = &source[source
            .find("fn plan_test_cut(")
            .expect("the test cut has its own function")..];
        assert!(
            body.find("choice.depth").expect("a depth is taken")
                < body.find("fn plan_engrave_spans").unwrap_or(body.len()),
            "the test cut must take the nominal engrave depth, not a narrowed span's",
        );
    }
}

/// The job's coordinate frame: what claims room outside the board, and how the claims combine.
#[cfg(test)]
mod job_frame_tests {
    use super::*;
    use crate::runtime::tooling::StepRaw;

    fn step(ops: &[&str]) -> StepRaw {
        StepRaw {
            name: "Step".into(),
            operations: ops.iter().map(|s| s.to_string()).collect(),
            cnc_id: None,
            fixture_id: None,
            toolset_id: None,
            drill: Default::default(),
            route_board: Default::default(),
            route_cutouts: Default::default(),
            engrave_copper: Default::default(),
            machines_back: false,
            pin_diameter: None,
        }
    }

    fn engraving(test_cut: bool) -> StepRaw {
        let mut s = step(&["engrave_copper"]);
        s.engrave_copper.test_cut = test_cut;
        s
    }

    fn pinning(diameter_mm: f64) -> StepRaw {
        let mut s = step(&["drill_locating_pins"]);
        s.pin_diameter = Some(Length::from_mm(diameter_mm));
        s
    }

    /// A near-left, page-turn fixture that declares a work clearance. Only the four fields the
    /// frame reads matter; the Z model is filled with the schema's own defaults so the shape is
    /// a plausible profile rather than a stub.
    fn fixture(clearance_mm: f64) -> FixtureProfile {
        FixtureProfile {
            id: "fixture".into(),
            name: "Test fixture".into(),
            backing_board: "clamps".into(),
            backboard_thickness: Length::from_mm(2.5),
            bed_clearance: Length::from_mm(0.5),
            breakthrough: Length::from_mm(0.5),
            z_retract: Length::from_mm(5.0),
            z_safe: Length::from_mm(20.0),
            origin_x0: "left".into(),
            origin_y0: "near".into(),
            work_clearance_x: Length::from_mm(clearance_mm),
            work_clearance_y: Length::from_mm(clearance_mm),
            board_flip_axis: "y".into(),
            origin_reference: "G55".into(),
            pending_required_fields: Default::default(),
            usable: true,
        }
    }

    /// A step that routes the board outline with the given kerf and finishing allowance.
    fn routing(kerf_mm: f64, finishing_mm: f64) -> StepRaw {
        let mut s = step(&["route_board"]);
        s.route_board.cut = "route".into();
        s.route_board.kerf = Length::from_mm(kerf_mm);
        s.route_board.finishing = Length::from_mm(finishing_mm);
        s
    }

    /// **Example 1 — a board that is only engraved sits at the clearance.** Nothing is cut
    /// outside it, so there is no extent to clear and the frame is the operator's own number
    /// and nothing else.
    #[test]
    fn a_job_that_only_engraves_puts_the_board_at_the_clearance() {
        let frame = job_frame(&[engraving(false)], Some(&fixture(5.0)));
        assert_eq!((frame.margin.x_min, frame.margin.y_min), (5.0, 5.0));
        assert_eq!((frame.margin.x_max, frame.margin.y_max), (0.0, 0.0));
        assert!(frame.test_band.is_none());
        assert!(frame.waste.is_none());
    }

    /// **Example 2 — an edge cut moves the board out by the material it removes.** The router
    /// takes `kerf + finishing` off the waste side, and that band has to clear the zero like
    /// anything else the program cuts.
    #[test]
    fn adding_an_edge_cut_moves_the_board_out_by_the_material_it_removes() {
        let frame = job_frame(&[engraving(false), routing(2.0, 0.1)], Some(&fixture(5.0)));
        assert!((frame.margin.x_min - 7.1).abs() < 1e-9, "got {}", frame.margin.x_min);
        assert!((frame.margin.y_min - 7.1).abs() < 1e-9, "got {}", frame.margin.y_min);
        assert_eq!(frame.waste, Some(Length::from_mm(2.1)));
    }

    /// **Example 3 — the test cut is free when the edge cut already leaves waste.** The band it
    /// needs for a 0.25 mm trough is 0.75 mm; the router leaves 2.1 mm of material it is going
    /// to remove anyway. Growing the frame for it would cost blank, move every coordinate in
    /// the job, and buy nothing.
    #[test]
    fn a_test_cut_costs_nothing_when_the_edge_cut_already_leaves_waste() {
        let f = fixture(5.0);
        let without = job_frame(&[engraving(false), routing(2.0, 0.1)], Some(&f));
        let with = job_frame(&[engraving(true), routing(2.0, 0.1)], Some(&f));

        assert_eq!(with.margin, without.margin, "the frame must not grow");
        assert_eq!(with.test_band, Some(Length::from_mm(2.1)), "it reuses the routed band");
    }

    /// With no outline to route there is no waste to borrow, so a band is opened — three trough
    /// widths, leaving a trough of material either side of the cut — and the board moves out by
    /// exactly that and no more.
    #[test]
    fn a_test_cut_without_edge_routing_opens_its_own_band() {
        let mut engrave = engraving(true);
        engrave.engrave_copper.width = Length::from_mm(0.25);
        let frame = job_frame(&[engrave], Some(&fixture(5.0)));

        assert_eq!(frame.test_band, Some(Length::from_mm(0.75)));
        assert!((frame.margin.x_min - 5.75).abs() < 1e-9, "got {}", frame.margin.x_min);
    }

    /// **The extents take the widest claim and do not sum.** They are all measured from the same
    /// edge and overlap in the material, so a job with pins *and* routing must not pay for both
    /// — that would charge the frame twice for one piece of blank and push the board further
    /// from the zero than anything needs.
    #[test]
    fn the_extents_take_the_widest_claim_and_do_not_sum() {
        let f = fixture(2.0);
        let pins_only = job_frame(&[pinning(3.2)], Some(&f));
        let route_only = job_frame(&[routing(2.0, 0.1)], Some(&f));
        let both = job_frame(&[pinning(3.2), routing(2.0, 0.1)], Some(&f));

        // 1.5 x 3.2 = 4.8 on the flip axis; 2.1 of routed waste all round.
        assert!((pins_only.margin.y_min - (2.0 + 4.8)).abs() < 1e-9);
        assert!((route_only.margin.y_min - (2.0 + 2.1)).abs() < 1e-9);
        assert!(
            (both.margin.y_min - (2.0 + 4.8)).abs() < 1e-9,
            "the wider claim wins; summing would give {}",
            2.0 + 4.8 + 2.1,
        );
        // And on the axis the pins do not grow, the routing is what has to clear.
        assert!((both.margin.x_min - (2.0 + 2.1)).abs() < 1e-9, "got {}", both.margin.x_min);
    }

    /// **The clearance is added outside every extent, never merged with one.** Taking the wider
    /// of the two there would let a 4.8 mm pin band swallow a 2 mm clearance whole and put a
    /// drilled hole's edge on the origin — the one thing the clearance exists to stop.
    #[test]
    fn the_clearance_is_added_outside_every_extent() {
        for clearance in [0.0, 2.0, 5.0] {
            let frame = job_frame(&[pinning(3.2), routing(2.0, 0.1)], Some(&fixture(clearance)));
            assert!(
                (frame.margin.y_min - (clearance + 4.8)).abs() < 1e-9,
                "C={clearance}: got {}",
                frame.margin.y_min,
            );
        }
    }

    /// **A step with the option set but no engraving claims nothing.** Every step carries a
    /// materialised `engrave_copper` block whether or not it engraves, so reading the flag
    /// without checking the operation would open a band for a job that never cuts copper.
    #[test]
    fn a_step_that_does_not_engrave_cannot_claim_a_test_cut_band() {
        let mut drilling = step(&["drill_pth"]);
        drilling.engrave_copper.test_cut = true;
        assert!(job_frame(&[drilling], Some(&fixture(2.0))).test_band.is_none());
    }

    /// **The pins can never fail to fit.** They shift the bounding box rather than competing for
    /// a fixed allowance, so however large the pin and however small the clearance, the hole
    /// stays wholly on the work side of the zero. That is the invariant the model was chosen
    /// for, and nothing else pins it.
    #[test]
    fn the_pins_can_never_fail_to_fit_whatever_the_clearance() {
        for clearance in [0.0, 0.5, 2.0, 10.0] {
            for diameter in [1.0, 2.0, 3.2, 6.0] {
                let frame = job_frame(&[pinning(diameter)], Some(&fixture(clearance)));
                assert!(
                    (frame.margin.y_min - (clearance + 1.5 * diameter)).abs() < 1e-9,
                    "C={clearance} D={diameter}: got {}",
                    frame.margin.y_min,
                );
            }
        }
    }

    /// **One step's option moves every step's zero.** The frame is job-wide because the zero is:
    /// a test cut in the engraving step shifts the drilling step's coordinates too, and any
    /// other answer would have the operator set up against two different origins in one job.
    #[test]
    fn a_test_cut_in_one_step_frames_the_whole_job() {
        let f = fixture(2.0);
        let with = job_frame(&[engraving(true), step(&["drill_pth"])], Some(&f));
        let without = job_frame(&[engraving(false), step(&["drill_pth"])], Some(&f));
        assert_ne!(with.margin, without.margin, "the frame has to notice");

        // And it does not matter which step asks.
        let later = job_frame(&[step(&["drill_pth"]), engraving(true)], Some(&f));
        assert_eq!(later.margin, with.margin);
    }

    /// A fixture that resolves to nothing at all leaves the frame inert, which is what every
    /// "a job without any of this is unchanged" property rests on.
    #[test]
    fn no_fixture_and_no_claims_is_an_inert_frame() {
        assert_eq!(job_frame(&[step(&["drill_pth"])], None).margin, Margin::default());
    }
}


/// The order the blocks come out in, guarded at the source.
///
/// Block order is **push order** — there is no sort, no `Phase` comparison, nothing that
/// would fail to compile if two pushes were swapped. So the sequence a board is made in
/// lives in the order of a few statements, and these are what stop an innocuous-looking
/// edit reordering the program.
#[cfg(test)]
mod block_order_tests {
    /// The body of `plan_step`, from the engrave block to the end.
    fn body() -> &'static str {
        let source = include_str!("machining_plan.rs");
        source
            .split_once("    let mut blocks = Vec::new();")
            .expect("plan_step builds its blocks in one place")
            .1
    }

    fn at(needle: &str) -> usize {
        body().find(needle).unwrap_or_else(|| panic!("{needle:?} is not in plan_step"))
    }

    /// **Engraving is the first thing in the program.** Z0 is verified against an
    /// unmachined surface, so the program has to open by selecting the engraving tool —
    /// which it does by the engrave block being pushed before any other.
    ///
    /// It is also the right order physically: the copper is cut while the board is whole,
    /// flat and undrilled, and every hole made first is a place the surface can lift or the
    /// bit can catch.
    #[test]
    fn the_engrave_block_is_pushed_before_any_other() {
        let engrave = at("blocks.extend(engraved);");

        for later in [
            "blocks.extend(plan_drilling(",
            "blocks.extend(plan_routing(",
            "blocks.extend(plan_outline(",
        ] {
            assert!(
                engrave < at(later),
                "`{later}` is pushed before the engrave block, so the program would not \
                 open with the engraving tool",
            );
        }
    }

    /// **Drilling before routing** — a hard constraint, not a preference (op-planner §4.1).
    /// Routing releases the part, so all drilling must finish while the board is fully
    /// attached and flat.
    #[test]
    fn every_hole_is_drilled_before_anything_is_routed() {
        assert!(at("blocks.extend(plan_drilling(") < at("blocks.extend(plan_routing("));
        assert!(at("blocks.extend(plan_routing(") < at("blocks.extend(plan_outline("));
    }

    /// **Interior cutouts before the perimeter, on their own cutter too.**
    ///
    /// The cutouts sharing the outline's cutter are ordered by the pass list inside one
    /// block; the ones on any *other* cutter are separate blocks, and those have to be
    /// pushed first. They used to be pushed last — after the outline had already released
    /// the part — which is the same fault the pass list fixes, in the other half of the
    /// problem.
    #[test]
    fn cutouts_on_their_own_cutter_are_cut_before_the_outline() {
        let loop_start = at("for (router_id, spans) in &cutout_spans {");
        let outline_block = at("if let Some(outline_router) = routers.outline.as_deref() {");

        assert!(
            loop_start < outline_block,
            "cutout blocks on other cutters must be pushed before the outline block",
        );
    }

    /// And the shared cutter's cutouts lead the outline's own passes within their block.
    /// `plan_outline` runs passes in the order given, so this array *is* the cut order.
    #[test]
    fn the_shared_cutters_cutouts_lead_the_outline_passes() {
        let passes = body()
            .split_once("blocks.extend(plan_outline(")
            .expect("the outline block is planned here")
            .1;
        let order: Vec<usize> = [
            "&shared_cutout_rough",
            "&shared_cutout_spans",
            "&outline_rough",
            "&outline_spans",
        ]
        .iter()
        .map(|name| passes.find(name).unwrap_or_else(|| panic!("{name} is a pass")))
        .collect();

        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "the cutout passes must precede the outline passes: {order:?}",
        );
    }
}
