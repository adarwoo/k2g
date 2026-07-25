//! Operation-planner adapter — builds the in-memory [`MachiningPlan`] the Job
//! "Machining" view renders (operation-planner.md). It resolves each machining step
//! (operations, drill config, toolset, CNC), runs the **same** tool assignment as the
//! Tooling tab (so the machining blocks and the rack agree), then hands the resolved
//! drill targets to the pure [`planner`](crate::gcode::planner) for decomposition and
//! ordering.
//!
//! **Scope:** the drill phase. Round PTH/NPTH holes (and vias) that resolve to a drill
//! become ordered point-drill ops; oblong slots, routed holes and board-outline
//! routing are recorded as pending notes and planned once the stitcher preserves typed
//! segments (op-planner §3, §9.6). Heights (`z_retract`/`z_safe`) use provisional
//! defaults until the fixture model carries them.

use uuid::Uuid;

use units::Length;

use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{self, AssignConfig, AssignError, Strategy, Weights};
use crate::gcode::placement::Placement;
use crate::gcode::plan::{MachiningPlan, Point, StepPlan};
use crate::gcode::oblong;
use crate::gcode::planner::{plan_drilling, plan_routing, DrillTarget, RouteTarget};
use crate::runtime::tooling::{
    build_rack_spec, build_setup, collect_hole_groups, plan_routers, read_steps, HoleGroup,
    OblongStrategy, StepRaw,
};
use crate::runtime::AppState;

/// R-plane retract used only when a step has no resolvable fixture (the fixture now
/// carries `z_retract`); keeps the Machining view planning mid-configuration (mm).
const DEFAULT_Z_RETRACT_MM: f64 = 2.0;
/// Safe height used only when a step has no resolvable fixture (the fixture now
/// carries `z_safe`); keeps the Machining view planning mid-configuration (mm).
const DEFAULT_Z_SAFE_MM: f64 = 5.0;

/// Builds the machining plan for the current context: one [`StepPlan`] per machining
/// step of the selected profile, each with its ordered drill-phase tool blocks.
pub fn plan_machining(ctx: &AppState) -> MachiningPlan {
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

/// A whole-plan note (nothing to plan).
fn note(message: &str) -> MachiningPlan {
    MachiningPlan { steps: vec![], note: Some(message.to_string()) }
}

/// Plans one step's drill phase.
fn plan_step(ctx: &AppState, index: usize, raw: &StepRaw, orientation: f64) -> StepPlan {
    let name = raw.name.clone();
    let mut notes: Vec<String> = Vec::new();

    let has_pth = raw.operations.iter().any(|op| op == "drill_pth");
    let has_npth = raw.operations.iter().any(|op| op == "drill_npth");
    let has_route = raw.operations.iter().any(|op| op == "route_board" || op == "mill_board");
    let has_locating = raw.operations.iter().any(|op| op == "drill_locating_pins");

    // Resolve the toolset (rack) and CNC (ATC + scaling).
    let Some(toolset_id) = raw.toolset_id else {
        return failed(index, name, vec!["This step has no toolset selected.".into()]);
    };
    let Some(toolset) = ctx.toolsets.iter().find(|t| t.id == toolset_id.to_string()) else {
        return failed(index, name, vec!["The step's toolset profile could not be found.".into()]);
    };
    let cnc = raw.cnc_id.and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()));
    let atc_slots = cnc.map(|m| m.atc_slot_count as usize).unwrap_or(0);
    let fixture = raw
        .fixture_id
        .and_then(|id| ctx.fixtures.iter().find(|f| f.id == id.to_string()));

    let holes: &[pcb::BoardHole] = ctx.board.as_ref().map(|b| b.holes.as_slice()).unwrap_or(&[]);
    let groups = collect_hole_groups(holes, has_pth, has_npth);

    // The rack must reserve every router routing requires — the outline cutter and one
    // per slot width, since a cutter wider than a slot cannot mill it. Resolved by the
    // shared planner so this and the Tooling tab produce the same rack, and thus the
    // same slot numbers.
    let has_oblongs = groups.iter().any(|g| g.minor.is_some());
    let oblong = OblongStrategy::from_key(&raw.drill.oblong);
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
    let mut pending_slot_routes = 0usize;
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
            if oblong.routes() {
                pending_slot_routes += 1;
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
                hole_diameter: group.target,
                z_bottom: assigned.z_bottom,
            });
        }
    }

    // Place ops in machine space and order each phase: drilling first (board rigid),
    // then the route-hole phase (op-planner §4).
    let placement = Placement::new(
        ctx.board.as_ref().and_then(|b| b.bounding_box.as_ref()),
        orientation,
        cnc.map(|m| m.scaling_x as f64).unwrap_or(1.0),
        cnc.map(|m| m.scaling_y as f64).unwrap_or(1.0),
        fixture.map(|f| f.z_retract).unwrap_or(Length::from_mm(DEFAULT_Z_RETRACT_MM)),
        fixture.map(|f| f.z_safe).unwrap_or(Length::from_mm(DEFAULT_Z_SAFE_MM)),
    );
    let start = Point::new(Length::from_mm(0.0), Length::from_mm(0.0));
    let mut blocks = plan_drilling(&drill_targets, &placement, start, &slots);
    blocks.extend(plan_routing(&route_targets, &placement, start, &slots));

    // Record what this step's plan does not yet cover.
    if pending_slot_routes > 0 {
        notes.push(format!(
            "{pending_slot_routes} oblong slot(s) still need their route pass — awaits the route phase. \
             Any drilling their strategy calls for is planned."
        ));
    }
    // Surface unmillable slots here too: when the oblong phase lands these are the
    // features that will have no tool, and the Tooling tab carries the full detail.
    if !routers.unroutable_widths.is_empty() {
        notes.push(format!(
            "{} slot width(s) are narrower than any available router — see the Tooling tab.",
            routers.unroutable_widths.len()
        ));
    }
    if has_route {
        notes.push("Board-outline routing not yet planned — awaits the outline route phase.".into());
    }
    if has_locating {
        notes.push("Locating pins are not yet planned.".into());
    }
    for diagnostic in &assignment.diagnostics {
        notes.push(diagnostic.message.clone());
    }

    StepPlan { index, name, blocks, notes }
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
