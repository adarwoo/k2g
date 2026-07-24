//! Tooling-plan adapter: runs the tool-selection [`assigner`](crate::gcode::assigner)
//! for **each machining step** of the selected profile and shapes the result for the
//! Job screen's "Tooling" tab.
//!
//! Per-step data (operations, bindings, drill config) lives only in the datastore
//! document — the in-memory `JobProfile` projection is step-0 flattened — so this
//! reads `/steps/{i}/…` directly via [`with_appdata`], resolves the CNC/toolset, maps
//! the board holes to the assigner's `HoleDemand`, and calls `assign()`.
//!
//! Scope note: outline **routing tool selection** is preliminary (a heuristic pick of
//! an available router), and the **bed-collision** half of Z-feasibility is relaxed
//! here — the fixture model does not yet carry the board-to-bed space, so only the
//! *reach* check (flute length vs. required plunge) is enforced. Both firm up when the
//! geometry pre-pass and fixture geometry land (see the plan's later phases).

use uuid::Uuid;

use datastore::{Node, NodeValue, UnitValue};
use units::Length;
use units::UserUnitDisplay;

use crate::data::model::tool_core::ToolKind;
use crate::data::model::{Tool, ToolsetGenerationPolicy};
use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{
    self, Allowance, AssignConfig, AssignError, DemandKind, FaultReason, HoleDemand, OverflowPolicy,
    RackSpec, Setup, Strategy, ToolAssignment, Weights,
};
use crate::runtime::AppState;

/// The full tooling plan: one entry per machining step (in order).
pub struct ToolingPlan {
    pub steps: Vec<StepPlan>,
    /// A top-level note when there is nothing to plan (no profile / no board).
    pub note: Option<String>,
}

pub struct StepPlan {
    pub index: usize,
    pub name: String,
    pub outcome: StepOutcome,
}

pub enum StepOutcome {
    /// Nothing to machine in this step (no drillable holes and no routing).
    Empty,
    /// A resolved plan — the rack and the per-requirement resolution.
    Resolved(StepResolved),
    /// No solution — the diagnostic lines to display as an error.
    Failed(Vec<String>),
}

pub struct StepResolved {
    /// One-line context, e.g. "9 tools · manual tool changes (no ATC)".
    pub summary: String,
    pub rack: Vec<RackRow>,
    pub requirements: Vec<RequirementRow>,
    pub warnings: Vec<String>,
}

pub struct RackRow {
    /// Slot label, e.g. `T1`.
    pub slot: String,
    pub tool: String,
}

pub struct RequirementRow {
    pub label: String,
    pub count: usize,
    /// The tool(s) resolving this requirement. An oblong/slot may use two — a drill
    /// for the ends/width plus a router for the slot.
    pub tools: Vec<ResolvedTool>,
}

/// One tool resolving (part of) a requirement, for the plan table.
pub struct ResolvedTool {
    /// Slot label, e.g. `T3`, or `—` when unresolved.
    pub slot: String,
    /// Role when a requirement uses several tools ("drill"); else `None`.
    pub role: Option<&'static str>,
    /// Selected tool diameter (formatted), or `—`.
    pub diameter: String,
    /// Size delta: `+3.2%`, `exact` (routed to size), or `—`.
    pub delta_text: String,
    /// CSS class colouring the delta by magnitude.
    pub delta_class: &'static str,
    /// This feature is milled by a router rather than drilled.
    pub routed: bool,
}

/// Raw per-step data read from the datastore in one pass (owned so the `with_appdata`
/// lock is released before the assigner runs). Shared with the operation-planner
/// adapter ([`crate::runtime::machining_plan`]), which reads the same steps.
pub(crate) struct StepRaw {
    pub(crate) name: String,
    pub(crate) operations: Vec<String>,
    pub(crate) cnc_id: Option<Uuid>,
    pub(crate) toolset_id: Option<Uuid>,
    pub(crate) drill: DrillConfigRaw,
}

/// The step's drill `holes` config, defaulted when absent.
pub(crate) struct DrillConfigRaw {
    pub(crate) route_fallback: bool,
    pub(crate) drill_first: bool,
    pub(crate) pilot: bool,
    /// Oblong-hole strategy: `route | drill_ends_then_route | drill_chain | drill_chain_then_route`.
    pub(crate) oblong: String,
    pub(crate) oversize: Allowance,
    pub(crate) undersize: Allowance,
}

impl Default for DrillConfigRaw {
    fn default() -> Self {
        Self {
            route_fallback: false,
            drill_first: true,
            pilot: false,
            oblong: "drill_ends_then_route".to_string(),
            oversize: Allowance { relative: 0.08, max: Length::from_mm(0.10) },
            undersize: Allowance { relative: 0.06, max: Length::from_mm(0.08) },
        }
    }
}

/// Builds the tooling plan for the current context. Reads all steps of the selected
/// machining profile, runs the assigner for each, and formats the outcome.
pub fn plan_tooling(ctx: &AppState) -> ToolingPlan {
    let Some(profile_id) = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return ToolingPlan { steps: vec![], note: Some("Select a machining profile to plan tooling.".into()) };
    };
    if ctx.board.is_none() {
        return ToolingPlan { steps: vec![], note: Some("No board loaded — nothing to machine.".into()) };
    }
    if !appdata_ready() {
        return ToolingPlan { steps: vec![], note: Some("Configuration store is not ready.".into()) };
    }

    let raw_steps = read_steps(profile_id);
    if raw_steps.is_empty() {
        return ToolingPlan { steps: vec![], note: Some("The machining profile has no steps.".into()) };
    }

    let steps = raw_steps
        .into_iter()
        .enumerate()
        .map(|(index, raw)| StepPlan {
            index,
            name: raw.name.clone(),
            outcome: plan_step(ctx, &raw),
        })
        .collect();

    ToolingPlan { steps, note: None }
}

/// Reads every step's operations, bindings and drill config from the profile document.
pub(crate) fn read_steps(profile_id: Uuid) -> Vec<StepRaw> {
    with_appdata(|data| {
        let Some(doc) = data.get(profile_id) else {
            return Vec::new();
        };
        let root = &doc.root;
        let count = match root.get_pointer("/steps").map(|n| &n.value) {
            Some(NodeValue::Array(items)) => items.len(),
            _ => 0,
        };

        (0..count)
            .map(|i| {
                let operations = node_operations(root, &format!("/steps/{i}/operations"));
                // Read drill config from whichever drill op is present.
                let drill_base = if operations.iter().any(|op| op == "drill_pth") {
                    Some(format!("/steps/{i}/drill_pth/holes"))
                } else if operations.iter().any(|op| op == "drill_npth") {
                    Some(format!("/steps/{i}/drill_npth/holes"))
                } else {
                    None
                };
                let drill = drill_base
                    .map(|base| read_drill_config(root, &base))
                    .unwrap_or_default();

                StepRaw {
                    name: node_str(root, &format!("/steps/{i}/name"))
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| format!("Step {}", i + 1)),
                    operations,
                    cnc_id: node_ref(root, &format!("/steps/{i}/cnc/default")),
                    toolset_id: node_ref(root, &format!("/steps/{i}/toolset/default")),
                    drill,
                }
            })
            .collect()
    })
}

/// Reads the `holes` drill config at `base`, falling back to defaults per field.
fn read_drill_config(root: &Node, base: &str) -> DrillConfigRaw {
    let default = DrillConfigRaw::default();
    DrillConfigRaw {
        route_fallback: node_bool(root, &format!("{base}/route_fallback")).unwrap_or(default.route_fallback),
        drill_first: node_bool(root, &format!("{base}/drill_first")).unwrap_or(default.drill_first),
        pilot: node_bool(root, &format!("{base}/pilot")).unwrap_or(default.pilot),
        oblong: node_str(root, &format!("{base}/oblong")).unwrap_or(default.oblong),
        oversize: read_allowance(root, &format!("{base}/oversize"), default.oversize),
        undersize: read_allowance(root, &format!("{base}/undersize"), default.undersize),
    }
}

/// Reads an `{relative, max}` allowance, defaulting each field.
fn read_allowance(root: &Node, base: &str, fallback: Allowance) -> Allowance {
    Allowance {
        relative: node_percent_fraction(root, &format!("{base}/relative")).unwrap_or(fallback.relative),
        max: node_length(root, &format!("{base}/max")).unwrap_or(fallback.max),
    }
}

/// Plans one step: builds demands + rack, runs the assigner, formats the outcome.
fn plan_step(ctx: &AppState, raw: &StepRaw) -> StepOutcome {
    let has_pth = raw.operations.iter().any(|op| op == "drill_pth");
    let has_npth = raw.operations.iter().any(|op| op == "drill_npth");
    let has_route = raw.operations.iter().any(|op| op == "route_board" || op == "mill_board");
    let has_locating = raw.operations.iter().any(|op| op == "drill_locating_pins");

    // Resolve the toolset (rack) and CNC (ATC capacity).
    let Some(toolset_id) = raw.toolset_id else {
        return StepOutcome::Failed(vec!["This step has no toolset selected.".into()]);
    };
    let Some(toolset) = ctx.toolsets.iter().find(|t| t.id == toolset_id.to_string()) else {
        return StepOutcome::Failed(vec!["The step's toolset profile could not be found.".into()]);
    };
    let atc_slots = raw
        .cnc_id
        .and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()))
        .map(|m| m.atc_slot_count as usize)
        .unwrap_or(0);

    // Build the hole demand set from the board, grouped by (kind, size) for counts.
    let holes = ctx.board.as_ref().map(|b| b.holes.as_slice()).unwrap_or(&[]);
    let groups = collect_hole_groups(holes, has_pth, has_npth);

    // A router is needed for the board outline and/or for oblong slots that route.
    let has_oblongs = groups.iter().any(|g| g.minor.is_some());
    let oblong_drills = matches!(
        raw.drill.oblong.as_str(),
        "drill_ends_then_route" | "drill_chain" | "drill_chain_then_route"
    );
    let oblong_routes = matches!(
        raw.drill.oblong.as_str(),
        "route" | "drill_ends_then_route" | "drill_chain_then_route"
    );
    let needs_router = has_route || (has_oblongs && oblong_routes);

    let mut warnings: Vec<String> = Vec::new();
    let outline_router = if needs_router { pick_outline_router(ctx, toolset) } else { None };
    if needs_router && outline_router.is_none() {
        warnings.push("No router in stock for routing (board outline / slots) — routing is unresolved.".into());
    }
    if has_locating {
        warnings.push("Locating pins are not yet planned (no board metadata for locating holes).".into());
    }

    if groups.is_empty() && !has_route {
        return StepOutcome::Empty;
    }

    // Assemble the assigner inputs.
    let demands: Vec<HoleDemand> = groups.iter().map(|g| g.to_demand()).collect();
    let cfg = AssignConfig {
        allow_routing_holes: raw.drill.route_fallback,
        drill_first: raw.drill.drill_first,
        pilot: raw.drill.pilot,
        oversize: raw.drill.oversize,
        undersize: raw.drill.undersize,
        weights: Weights::default(),
    };
    let setup = Setup {
        board_thickness: ctx.board.as_ref().and_then(|b| b.thickness).unwrap_or(Length::from_mm(1.6)),
        // Bed-collision is relaxed until the fixture models the under-board space; the
        // reach check (below) still enforces that a bit can plunge through the board.
        bed_clearance: Length::from_mm(1_000.0),
        breakthrough_margin: Length::from_mm(0.5),
    };
    let rack = build_rack_spec(toolset, atc_slots, outline_router.as_deref());

    match assigner::assign(&demands, &ctx.tools, &cfg, &rack, &setup) {
        Ok(assignment) => {
            // The assigner already placed each tool on the toolset's real slot (fixed
            // tools pinned; the rest filling spare slots in order; do-not-use slots
            // skipped), so the slot numbers are used as-is.
            let number_of: std::collections::BTreeMap<&str, u8> =
                assignment.rack.iter().map(|s| (s.tool_id.as_str(), s.slot)).collect();

            let rack_rows: Vec<RackRow> = assignment
                .rack
                .iter()
                .map(|s| RackRow { slot: format!("T{}", s.slot), tool: tool_label(ctx, &s.tool_id) })
                .collect();

            let mut requirements: Vec<RequirementRow> = groups
                .iter()
                .map(|group| {
                    let tools = resolve_group_tools(
                        ctx,
                        &assignment,
                        group,
                        &number_of,
                        oblong_drills,
                        oblong_routes,
                        outline_router.as_deref(),
                    );
                    RequirementRow { label: group.label(ctx), count: group.count, tools }
                })
                .collect();

            if has_route {
                let router = outline_router
                    .as_ref()
                    .map(|id| resolve_router_tool(ctx, id, &number_of, None))
                    .unwrap_or_else(unresolved_tool);
                requirements.push(RequirementRow {
                    label: "Board outline (route)".into(),
                    count: 1,
                    tools: vec![router],
                });
            }

            for diagnostic in &assignment.diagnostics {
                warnings.push(diagnostic.message.clone());
            }

            StepOutcome::Resolved(StepResolved {
                summary: machine_summary(rack_rows.len(), atc_slots),
                rack: rack_rows,
                requirements,
                warnings,
            })
        }
        Err(error) => StepOutcome::Failed(format_error(ctx, &error)),
    }
}

/// Resolves the tool(s) for a requirement group: the assigner's drill (a round hole,
/// or the ends/width of an oblong) plus a router when the oblong strategy routes the
/// slot. A round hole is a single tool.
fn resolve_group_tools(
    ctx: &AppState,
    assignment: &ToolAssignment,
    group: &HoleGroup,
    number_of: &std::collections::BTreeMap<&str, u8>,
    oblong_drills: bool,
    oblong_routes: bool,
    outline_router: Option<&str>,
) -> Vec<ResolvedTool> {
    if group.minor.is_none() {
        return vec![resolve_drill_tool(ctx, assignment, group, number_of, None)];
    }
    // Oblong / slot: possibly a drill (ends or chain) and a router (the slot).
    let mut tools = Vec::new();
    if oblong_drills {
        tools.push(resolve_drill_tool(ctx, assignment, group, number_of, Some("drill")));
    }
    if oblong_routes {
        if let Some(router) = outline_router {
            tools.push(resolve_router_tool(ctx, router, number_of, None));
        }
    }
    if tools.is_empty() {
        tools.push(resolve_drill_tool(ctx, assignment, group, number_of, None));
    }
    tools
}

/// The drill the assigner picked for a group, with its diameter and size delta.
fn resolve_drill_tool(
    ctx: &AppState,
    assignment: &ToolAssignment,
    group: &HoleGroup,
    number_of: &std::collections::BTreeMap<&str, u8>,
    role: Option<&'static str>,
) -> ResolvedTool {
    let Some(assigned) = assignment.holes.iter().find(|h| h.hole_id == group.id()) else {
        return ResolvedTool { role, ..unresolved_tool() };
    };
    let slot = number_of.get(assigned.tool_id.as_str()).map(|n| format!("T{n}")).unwrap_or_else(|| "—".into());
    let diameter = ctx.tools.iter().find(|t| t.id == assigned.tool_id).map(|t| t.diameter);
    let match_len = group.minor.unwrap_or(group.target);
    let routed = assigned.strategy == Strategy::Route;
    match diameter {
        Some(dia) => {
            let (delta_text, delta_class) = if routed {
                ("exact".to_string(), "tooling-delta-ok")
            } else {
                delta_cell(dia, match_len)
            };
            ResolvedTool { slot, role, diameter: fmt_len(ctx, dia), delta_text, delta_class, routed }
        }
        None => ResolvedTool { slot, role, diameter: "—".into(), delta_text: "—".into(), delta_class: "", routed },
    }
}

/// A router resolving a routed slot / outline. It interpolates to the exact size, so
/// there is no size delta — the tool diameter is just the router's own.
fn resolve_router_tool(
    ctx: &AppState,
    router_id: &str,
    number_of: &std::collections::BTreeMap<&str, u8>,
    role: Option<&'static str>,
) -> ResolvedTool {
    let slot = number_of.get(router_id).map(|n| format!("T{n}")).unwrap_or_else(|| "—".into());
    let diameter = ctx
        .tools
        .iter()
        .find(|t| t.id == router_id)
        .map(|t| fmt_len(ctx, t.diameter))
        .unwrap_or_else(|| "—".into());
    ResolvedTool { slot, role, diameter, delta_text: "exact".into(), delta_class: "tooling-delta-ok", routed: true }
}

/// The size-delta cell for a drill of `tool` diameter making a `target`-size hole.
/// Computed at micron precision (matching the assigner) so an exact match reads
/// `exact` rather than a rounded `+0.0%`; otherwise `(tool − target) / target`,
/// green within 2 % and amber beyond, kept to enough precision that a real (if
/// tiny) difference never collapses to a misleading `0.0%`.
fn delta_cell(tool: Length, target: Length) -> (String, &'static str) {
    let target_um = micron(target);
    if target_um == 0 {
        return ("—".to_string(), "");
    }
    if micron(tool) == target_um {
        return ("exact".to_string(), "tooling-delta-ok");
    }
    let pct = (micron(tool) - target_um) as f64 / target_um as f64 * 100.0;
    let class = if pct.abs() < 2.0 { "tooling-delta-ok" } else { "tooling-delta-warn" };
    let text = if pct.abs() < 0.05 { format!("{pct:+.2}%") } else { format!("{pct:+.1}%") };
    (text, class)
}

fn unresolved_tool() -> ResolvedTool {
    ResolvedTool {
        slot: "—".into(),
        role: None,
        diameter: "—".into(),
        delta_text: "—".into(),
        delta_class: "",
        routed: false,
    }
}

/// A distinct machining requirement (holes of one kind and size) with its count.
/// Shared with the operation-planner adapter ([`crate::runtime::machining_plan`]),
/// which recomputes a single hole's group to match it back to the assignment.
pub(crate) struct HoleGroup {
    pub(crate) kind: DemandKind,
    /// The nominal size (the larger axis for an oblong).
    pub(crate) target: Length,
    /// The minor axis for an oblong hole; `None` for a round hole.
    pub(crate) minor: Option<Length>,
    pub(crate) count: usize,
}

impl HoleGroup {
    /// Classifies a single board hole into its requirement group (count 1), or `None`
    /// when the hole is not drilled by the enabled operations. This is the one place
    /// the kind/oblong classification lives, so [`collect_hole_groups`] and the
    /// operation-planner adapter agree on a hole's group (and thus its [`id`](Self::id)).
    pub(crate) fn from_hole(hole: &pcb::BoardHole, has_pth: bool, has_npth: bool) -> Option<HoleGroup> {
        let kind = match hole.kind {
            pcb::HoleKind::PadPth | pcb::HoleKind::Via if has_pth => DemandKind::Pth,
            pcb::HoleKind::PadNpth if has_npth => DemandKind::Npth,
            _ => return None,
        };
        let dx = hole.drill_x.or(hole.drill_y)?;
        let dy = hole.drill_y.or(hole.drill_x)?;
        let (major, minor_val) = if dx.as_mm() >= dy.as_mm() { (dx, dy) } else { (dy, dx) };
        let is_oblong = (micron(major) - micron(minor_val)).abs() > 1;
        let minor = if is_oblong { Some(minor_val) } else { None };
        Some(HoleGroup { kind, target: major, minor, count: 1 })
    }

    /// A stable identity used to match the assigner's per-hole result back to a group.
    pub(crate) fn id(&self) -> String {
        format!(
            "{}-{}-{}",
            kind_key(self.kind),
            micron(self.target),
            self.minor.map(micron).unwrap_or(-1)
        )
    }

    pub(crate) fn to_demand(&self) -> HoleDemand {
        HoleDemand {
            id: self.id(),
            kind: self.kind,
            target: self.target,
            minor_axis: self.minor,
            plated: matches!(self.kind, DemandKind::Pth),
            routable: true,
        }
    }

    fn label(&self, ctx: &AppState) -> String {
        let kind = match self.kind {
            DemandKind::Pth => "PTH",
            DemandKind::Npth => "NPTH",
            DemandKind::Locating => "Locating",
            DemandKind::CornerRelief => "Corner relief",
        };
        match self.minor {
            Some(minor) => format!(
                "{kind} oblong {} × {}",
                fmt_len(ctx, self.target),
                fmt_len(ctx, minor)
            ),
            None => format!("{kind} hole ⌀{}", fmt_len(ctx, self.target)),
        }
    }
}

/// Groups the board's drilled holes (filtered by the enabled drill operations) into
/// distinct (kind, size) requirements with counts. A hole with unequal X/Y drill is an
/// oblong (major = larger axis, minor = smaller). Classification is delegated to
/// [`HoleGroup::from_hole`] so grouping and the operation-planner adapter never drift.
pub(crate) fn collect_hole_groups(holes: &[pcb::BoardHole], has_pth: bool, has_npth: bool) -> Vec<HoleGroup> {
    let mut groups: Vec<HoleGroup> = Vec::new();
    for hole in holes {
        let Some(group) = HoleGroup::from_hole(hole, has_pth, has_npth) else { continue };
        let target_um = micron(group.target);
        let minor_um = group.minor.map(micron).unwrap_or(-1);
        if let Some(existing) = groups.iter_mut().find(|g| {
            g.kind == group.kind && micron(g.target) == target_um && g.minor.map(micron).unwrap_or(-1) == minor_um
        }) {
            existing.count += 1;
        } else {
            groups.push(group);
        }
    }
    groups
}

/// Picks a preliminary outline router: a routerbit/end-mill already pinned in the
/// toolset's fixed slots, else the smallest in-stock router in the shop. Returns its
/// stock-tool id.
pub(crate) fn pick_outline_router(ctx: &AppState, toolset: &crate::data::model::ToolsetProfile) -> Option<String> {
    let is_router = |tool: &Tool| matches!(ToolKind::from_kind_label(&tool.kind), ToolKind::Routerbit | ToolKind::Endmill);

    // Prefer a router already fixed in the toolset.
    let fixed_router = toolset
        .slots
        .values()
        .filter_map(|slot| slot.tool_id.as_ref())
        .find_map(|id| ctx.tools.iter().find(|t| &t.id == id && is_router(t)));
    if let Some(tool) = fixed_router {
        return Some(tool.id.clone());
    }

    // Else the smallest in-stock router (safest for internal corners).
    ctx.tools
        .iter()
        .filter(|t| is_router(t) && t.status == crate::data::model::ToolStatus::InStock)
        .min_by_key(|t| micron(t.diameter))
        .map(|t| t.id.clone())
}

/// Maps a toolset + ATC count to the assigner's rack spec. Capacity = usable
/// (non-disabled) slots, capped by the ATC size when the machine has one.
pub(crate) fn build_rack_spec(
    toolset: &crate::data::model::ToolsetProfile,
    atc_slots: usize,
    outline_router: Option<&str>,
) -> RackSpec {
    let fixed: Vec<(u8, String)> = toolset
        .slots
        .iter()
        .filter_map(|(index, slot)| {
            if slot.locked && !slot.disabled {
                slot.tool_id.as_ref().map(|id| (*index, id.clone()))
            } else {
                None
            }
        })
        .collect();

    // Spare slots (index order) available for auto-assignment: neither fixed nor
    // do-not-use. A `BTreeMap` iterates by key, so these come out sorted.
    let spare_slots: Vec<u8> = toolset
        .slots
        .iter()
        .filter(|(_, slot)| !slot.locked && !slot.disabled)
        .map(|(index, _)| *index)
        .collect();

    // Capacity is the number of DISTINCT tools the rack can hold: each distinct fixed
    // tool plus one per spare slot. Counting physical slots would over-count when a
    // tool is fixed in several slots (the extra slots are wasted, not extra capacity),
    // letting the assigner "resolve" more tools than can actually be placed. An ATC
    // machine caps this by its physical slot count.
    let distinct_fixed = fixed
        .iter()
        .map(|(_, id)| id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let placeable = distinct_fixed + spare_slots.len();
    let capacity = if atc_slots > 0 { placeable.min(atc_slots) } else { placeable };

    let mandatory = outline_router.map(|id| vec![id.to_string()]).unwrap_or_default();

    RackSpec { capacity, fixed, spare_slots, mandatory, policy: map_policy(toolset.generation_policy) }
}

/// A one-line context for a resolved step: tool count and how tools are changed.
/// A 0-ATC machine changes tools manually (no physical rack), so the T-numbers are
/// a change sequence rather than slot positions.
fn machine_summary(tool_count: usize, atc_slots: usize) -> String {
    if atc_slots == 0 {
        format!("{tool_count} tool(s) · manual tool changes (no ATC)")
    } else {
        format!("{tool_count} tool(s) · {atc_slots}-slot ATC")
    }
}

fn map_policy(policy: ToolsetGenerationPolicy) -> OverflowPolicy {
    match policy {
        ToolsetGenerationPolicy::FixedToolset => OverflowPolicy::FixedToolset,
        ToolsetGenerationPolicy::AllowReload => OverflowPolicy::AllowReload,
        ToolsetGenerationPolicy::AllowHybrid => OverflowPolicy::AllowHybrid,
    }
}

/// Formats an assigner error into displayable diagnostic lines.
fn format_error(ctx: &AppState, error: &AssignError) -> Vec<String> {
    match error {
        AssignError::UncoverableHoles(faults) => faults
            .iter()
            .map(|fault| {
                let kind = match fault.kind {
                    DemandKind::Pth => "PTH",
                    DemandKind::Npth => "NPTH",
                    DemandKind::Locating => "Locating",
                    DemandKind::CornerRelief => "Corner-relief",
                };
                let reason = match fault.reason {
                    FaultReason::NoSizeMatch => "no in-stock drill matches within the allowance and routing is unavailable",
                    FaultReason::DepthInfeasible => "the matching drill is too short to reach through (or would hit the bed)",
                };
                let nearest = if fault.nearest.is_empty() {
                    String::new()
                } else {
                    format!(" — nearest stock: {}", fault.nearest.join(", "))
                };
                let size = fmt_len(ctx, Length::from_um(fault.target_um as f64));
                format!("{kind} hole ⌀{size}: {reason}{nearest}")
            })
            .collect(),
        AssignError::RackTooSmall { minimal, capacity } => vec![format!(
            "Rack too small: this step needs {minimal} tools but the toolset provides {capacity} usable slot(s). \
             Add slots, enable routing fallback, widen the size allowances, or drop optional operations."
        )],
    }
}

/// A stock tool's display label with its diameter.
fn tool_label(ctx: &AppState, tool_id: &str) -> String {
    match ctx.tools.iter().find(|t| t.id == tool_id) {
        Some(tool) => format!("{} (⌀{})", tool.display_name(), fmt_len(ctx, tool.diameter)),
        None => format!("Unknown tool ({tool_id})"),
    }
}

/// Formats a length in the user's preferred unit only (no native-unit suffix).
fn fmt_len(ctx: &AppState, length: Length) -> String {
    length.unit_display(ctx.unit_system).user
}

fn kind_key(kind: DemandKind) -> &'static str {
    match kind {
        DemandKind::Pth => "pth",
        DemandKind::Npth => "npth",
        DemandKind::Locating => "loc",
        DemandKind::CornerRelief => "corner",
    }
}

/// A length quantised to whole micrometres (matches the assigner's precision).
fn micron(length: Length) -> i64 {
    length.as_um().round() as i64
}

// --- datastore node readers ----------------------------------------------

fn node_bool(root: &Node, ptr: &str) -> Option<bool> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn node_str(root: &Node, ptr: &str) -> Option<String> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn node_ref(root: &Node, ptr: &str) -> Option<Uuid> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Ref(reference) => Some(reference.raw),
        NodeValue::Id(id) => Some(*id),
        NodeValue::Str(s) => Uuid::parse_str(s).ok(),
        _ => None,
    }
}

fn node_length(root: &Node, ptr: &str) -> Option<Length> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Unit(UnitValue::Length(length)) => Some(*length),
        NodeValue::Float(value) => Some(Length::from_mm(*value)),
        NodeValue::Int(value) => Some(Length::from_mm(*value as f64)),
        _ => None,
    }
}

/// Reads a `percent` value (stored untyped, usually `"8%"`) as a fraction (`0.08`).
fn node_percent_fraction(root: &Node, ptr: &str) -> Option<f64> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Str(s) => s.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0),
        NodeValue::Float(f) => Some(*f / 100.0),
        NodeValue::Int(i) => Some(*i as f64 / 100.0),
        _ => None,
    }
}

fn node_operations(root: &Node, ptr: &str) -> Vec<String> {
    match root.get_pointer(ptr).map(|n| &n.value) {
        Some(NodeValue::Array(items)) => items
            .iter()
            .filter_map(|item| match &item.value {
                NodeValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcb::{BoardHole, BoardPoint, HoleKind};

    fn hole(kind: HoleKind, dx_mm: f64, dy_mm: f64) -> BoardHole {
        BoardHole {
            id: None,
            kind,
            position: BoardPoint { x: Length::from_mm(0.0), y: Length::from_mm(0.0) },
            drill_x: Some(Length::from_mm(dx_mm)),
            drill_y: Some(Length::from_mm(dy_mm)),
            plated: None,
        }
    }

    #[test]
    fn groups_round_holes_by_size_and_counts_them() {
        let holes = vec![
            hole(HoleKind::PadPth, 0.8, 0.8),
            hole(HoleKind::PadPth, 0.8, 0.8),
            hole(HoleKind::Via, 0.3, 0.3),
        ];
        let groups = collect_hole_groups(&holes, true, false);
        assert_eq!(groups.len(), 2, "two distinct sizes");
        let g08 = groups.iter().find(|g| micron(g.target) == 800).unwrap();
        assert_eq!(g08.count, 2);
        assert!(g08.minor.is_none());
        assert_eq!(g08.kind, DemandKind::Pth);
    }

    #[test]
    fn detects_oblong_and_keeps_major_and_minor_axes() {
        let holes = vec![hole(HoleKind::PadNpth, 2.0, 4.0)];
        let groups = collect_hole_groups(&holes, false, true);
        assert_eq!(groups.len(), 1);
        assert_eq!(micron(groups[0].target), 4000, "major axis is the target");
        assert_eq!(groups[0].minor.map(micron), Some(2000), "minor axis retained");
        assert_eq!(groups[0].kind, DemandKind::Npth);
    }

    #[test]
    fn filters_holes_by_enabled_operation() {
        let holes = vec![hole(HoleKind::PadPth, 0.8, 0.8), hole(HoleKind::PadNpth, 3.0, 3.0)];
        // Only PTH enabled → the NPTH hole is excluded from the demand.
        let groups = collect_hole_groups(&holes, true, false);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DemandKind::Pth);
    }
}
