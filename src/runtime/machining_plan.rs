//! Operation-planner adapter — builds the in-memory [`MachiningPlan`] the Job
//! "Machining" view renders (operation-planner.md). It resolves each machining step
//! (operations, drill config, toolset, CNC), runs the **same** tool assignment as the
//! Tooling tab (so the machining blocks and the rack agree), then hands the resolved
//! drill targets to the pure [`planner`](crate::gcode::planner) for decomposition and
//! ordering.
//!
//! **Scope.** Both phases are planned. Round PTH/NPTH holes (and vias) become ordered
//! point-drill ops or spiral-routed pockets; oblong slots become drill chains, router
//! passes or both, per the step's strategy; and the board outline becomes offset cut
//! spans with retaining tabs left between them.
//!
//! Three things are deliberately still notes rather than ops: **locating pins** (the
//! board carries no metadata for them), **scoring / V-grooving** (partial-depth cuts need
//! a depth model and a V-bit the tool stock does not describe), and **arc-preserving
//! outline offsets** — the outline is offset as a polyline today, so a curved edge is cut
//! as chords rather than as `G2`/`G3` (op-planner §3, §9.6). None of these produces a
//! wrong program; each produces a less complete or less elegant one, and says so.
//!
//! Heights (`z_retract`/`z_safe`) use provisional defaults until the fixture model
//! carries them.

use uuid::Uuid;

use units::Length;

use crate::data::model::TabContour;
use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{self, AssignConfig, AssignError, Strategy, Weights};
use crate::gcode::placement::{BoardOrigin, Placement};
use crate::gcode::plan::{MachiningPlan, Point, StepPlan};
use crate::gcode::planner::{
    plan_drilling, plan_outline, plan_routing, DrillTarget, OutlineSpan, RouteShape, RouteTarget,
};
use crate::gcode::{oblong, outline, scene};
use crate::runtime::tooling::{
    build_rack_spec, build_setup, collect_hole_groups, missing_bindings, plan_routers, read_steps,
    HoleGroup, RouterPlan, StepRaw,
};
use crate::runtime::AppCtx;

/// Builds the machining plan for the current context: one [`StepPlan`] per machining
/// step of the selected profile, each with its ordered drill-phase tool blocks.
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

    let steps = raw_steps
        .into_iter()
        .enumerate()
        .map(|(index, raw)| plan_step(ctx, index, &raw, orientation))
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
    let raw = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(read_steps)
        .and_then(|steps| steps.into_iter().nth(step));
    let cnc = raw
        .as_ref()
        .and_then(|raw| raw.cnc_id)
        .and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()));
    let fixture = raw
        .as_ref()
        .and_then(|raw| raw.fixture_id)
        .and_then(|id| ctx.fixtures.iter().find(|f| f.id == id.to_string()));

    // Z here is irrelevant — a solid is placed in XY only — so the retract/safe heights
    // are nominal rather than resolved from a fixture.
    let placement = Placement::new(
        board.bounding_box.as_ref(),
        orientation,
        fixture
            .map(|f| BoardOrigin::from_edges(&f.origin_x0, &f.origin_y0))
            .unwrap_or_default(),
        cnc.map(|m| m.scaling_x as f64).unwrap_or(1.0),
        cnc.map(|m| m.scaling_y as f64).unwrap_or(1.0),
        Length::from_mm(0.0),
        Length::from_mm(0.0),
    );
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
        thickness_mm: board.thickness.map(|t| t.as_mm()).unwrap_or(DEFAULT_THICKNESS_MM),
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

/// Board thickness assumed when the KiCad stackup does not report one. 1.6 mm is the
/// overwhelmingly common PCB, and this only affects how the workpiece is *drawn*.
const DEFAULT_THICKNESS_MM: f64 = 1.6;

/// A whole-plan note (nothing to plan).
fn note(message: &str) -> MachiningPlan {
    MachiningPlan { steps: vec![], note: Some(message.to_string()) }
}

/// Plans one step's drill phase.
fn plan_step(ctx: &AppCtx, index: usize, raw: &StepRaw, orientation: f64) -> StepPlan {
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

    // Refused, not annotated. Nothing mirrors geometry for a bottom-side step, so every
    // block this would plan — and the drill map drawn from it — describes the top side.
    // Showing a plausible plan under a step labelled "Bottom" is the failure mode the
    // readiness gate exists to prevent, so the view must not draw one either.
    if raw.machines_bottom {
        return failed(
            index,
            name,
            vec![
                "This step is set to machine the bottom side, which is not implemented \
                 yet: no geometry is mirrored, so the program would be the top-side one. \
                 Set the step to the top side to plan it."
                    .into(),
            ],
        );
    }
    let (Some(cnc_id), Some(fixture_id), Some(toolset_id)) =
        (raw.cnc_id, raw.fixture_id, raw.toolset_id)
    else {
        unreachable!("missing_bindings just established all three are present")
    };
    let Some(toolset) = ctx.toolsets.iter().find(|t| t.id == toolset_id.to_string()) else {
        return failed(index, name, vec!["The step's toolset profile could not be found.".into()]);
    };
    let Some(cnc) = ctx.machines.iter().find(|m| m.id == cnc_id.to_string()) else {
        return failed(index, name, vec!["The step's CNC profile could not be found.".into()]);
    };
    let Some(fixture) = ctx.fixtures.iter().find(|f| f.id == fixture_id.to_string()) else {
        return failed(index, name, vec!["The step's fixture profile could not be found.".into()]);
    };
    let atc_slots = cnc.atc_slot_count as usize;

    // A fixture set up in an origin this controller does not have was once caught here, by
    // comparing the fixture's *ordinal* against a count on the CNC profile. Both are gone:
    // the fixture now names its origin the way the machine does, and the machine's
    // `set_origin` primitive is the single authority on which names it accepts — a count
    // could never have expressed a MASSO's `G54.1 P1..P100`. The check happens when the
    // program is generated, and refuses it outright rather than warning.

    let holes: &[pcb::BoardHole] = ctx.board.as_ref().map(|b| b.holes.as_slice()).unwrap_or(&[]);
    let groups = collect_hole_groups(holes, has_pth, has_npth);

    // The rack must reserve every router routing requires — the outline cutter and one
    // per slot width, since a cutter wider than a slot cannot mill it. Resolved by the
    // shared planner so this and the Tooling tab produce the same rack, and thus the
    // same slot numbers.
    let has_oblongs = groups.iter().any(|g| g.minor.is_some());
    let oblong = raw.oblong_strategy();
    let routers =
        plan_routers(&ctx.tools, toolset, &groups, has_route, has_oblongs && oblong.routes());

    if groups.is_empty() && !has_route {
        if has_locating {
            notes.push("Locating pins are not yet planned.".into());
        }
        return StepPlan { index, name, blocks: vec![], notes };
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
    let rack = build_rack_spec(toolset, atc_slots, &routers.mandatory_ids());

    let assignment = match assigner::assign(&demands, &ctx.tools, &cfg, &rack, &setup) {
        Ok(assignment) => assignment,
        Err(error) => return failed(index, name, format_assign_error(&error)),
    };

    // Tool id → rack slot, for the block's display.
    let slots: std::collections::BTreeMap<String, u8> =
        assignment.rack.iter().map(|s| (s.tool_id.clone(), s.slot)).collect();

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
        let Some(group) = HoleGroup::from_hole(hole, has_pth, has_npth) else { continue };
        let Some(assigned) = assignment.holes.iter().find(|h| h.hole_id == group.id()) else { continue };
        let Some(tool_diameter) = ctx.tools.iter().find(|t| t.id == assigned.tool_id).map(|t| t.diameter) else {
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
                        if router.flute_length.is_some_and(|f| f.as_mm() < z_bottom.as_mm()) {
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
                shape: RouteShape::Hole { hole_diameter: group.target },
                z_bottom: assigned.z_bottom,
            });
        }
    }

    // Place ops in machine space and order each phase: drilling first (board rigid),
    // then the route-hole phase (op-planner §4).
    let placement = Placement::new(
        ctx.board.as_ref().and_then(|b| b.bounding_box.as_ref()),
        orientation,
        BoardOrigin::from_edges(&fixture.origin_x0, &fixture.origin_y0),
        cnc.scaling_x as f64,
        cnc.scaling_y as f64,
        fixture.z_retract,
        fixture.z_safe,
    );
    let start = Point::new(Length::from_mm(0.0), Length::from_mm(0.0));

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

    let mut blocks = plan_drilling(&drill_targets, &placement, start, &slots);
    blocks.extend(plan_routing(&route_targets, &placement, start, &slots));
    if let Some(outline_router) = routers.outline.as_deref() {
        let tool = ctx.tools.iter().find(|t| t.id == outline_router);
        if let Some(tool) = tool {
            let z_bottom = assigner::router_plunge(&setup);
            if tool.flute_length.is_some_and(|f| f.as_mm() < z_bottom.as_mm()) {
                notes.push(format!(
                    "Outline router '{}' cannot reach through the board — the outline will \
                     not be cut free. Stock a longer cutter.",
                    tool.name
                ));
            }
            blocks.extend(plan_outline(
                &outline_spans,
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
            short_flute_routers.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    // Surface unmillable slots here too: the Tooling tab carries the full detail.
    if !routers.unroutable_widths.is_empty() {
        notes.push(format!(
            "{} slot width(s) are narrower than any available router — see the Tooling tab.",
            routers.unroutable_widths.len()
        ));
    }
    if has_locating {
        notes.push("Locating pins are not yet planned.".into());
    }
    for diagnostic in &assignment.diagnostics {
        notes.push(diagnostic.message.clone());
    }

    StepPlan { index, name, blocks, notes }
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
fn plan_outline_spans(
    ctx: &AppCtx,
    raw: &StepRaw,
    routers: &RouterPlan,
    placement: &Placement,
    drill_targets: &mut Vec<DrillTarget>,
    slots: &std::collections::BTreeMap<String, u8>,
) -> Result<(Vec<OutlineSpan>, Vec<String>), String> {
    let Some(stitched) = ctx.stitched_board_data.as_ref() else {
        return Err("The board outline has not been stitched yet — refresh the board snapshot.".into());
    };
    if !stitched.errors.is_empty() {
        return Err(format!(
            "The board outline could not be stitched into closed contours ({}), so it \
             cannot be routed.",
            stitched.errors.join("; ")
        ));
    }
    let Some(router_id) = routers.outline.as_deref() else {
        return Err("No router in stock for the board outline — the outline is not planned.".into());
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
    // Cutouts are numbered among themselves, as `job.yaml#/edge_tabs/index` means it.
    let (mut outer_n, mut cutout_n) = (0usize, 0usize);

    for (contour, offset) in stitched.contours.iter().zip(offsets) {
        let kind = if contour.is_hole { TabContour::Cutout } else { TabContour::Outer };
        let index = if contour.is_hole { &mut cutout_n } else { &mut outer_n };
        let (kind_index, label) = (*index, kind.as_str());
        *index += 1;

        // An interior opening the step chooses not to route: it stays as drawn copper,
        // and the board keeps the material.
        if contour.is_hole && !edge.cutouts {
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
        let Some(path) = outline::Loop::new(&points) else { continue };

        let retention = edge.retention(contour.is_hole);
        let width_mm = retention.width.as_mm();

        // Where the tabs go. Distribution runs on the contour's own **straight
        // segments**, not on the offset polyline — the offset flattens every rounded
        // corner into dozens of chords, so "segments" there would be meaningless. Each
        // computed anchor is then placed and projected onto the offset path, which for
        // an outward offset of a straight run is exactly the perpendicular foot.
        let tabs: Vec<f64> = if retention.tabs {
            let anchors = outline::distribute_tabs(
                &straight_segments_mm(contour),
                retention.count,
                width_mm,
            );
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

        for (n, span) in outline::cut_spans(&path, &tabs, width_mm).into_iter().enumerate() {
            spans.push(OutlineSpan { source: format!("{label}#{kind_index}.span{n}"), path: span });
        }

        // Mouse bites are drills, so they join the drill phase rather than the route one.
        if retention.mouse_bites {
            if let Some(bite_tool) = mouse_bite_drill(ctx, slots) {
                for (n, tab) in tabs.iter().enumerate() {
                    let centres =
                        outline::mouse_bite_centres(&path, *tab, width_mm, bite_tool.1);
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

/// A step that could not be planned — no blocks, the reasons surfaced as notes.
fn failed(index: usize, name: String, messages: Vec<String>) -> StepPlan {
    StepPlan { index, name, blocks: vec![], notes: messages }
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
