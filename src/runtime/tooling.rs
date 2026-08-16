//! Tooling-plan adapter: runs the tool-selection [`assigner`](crate::gcode::assigner)
//! for **each machining step** of the selected profile and shapes the result for the
//! Job screen's "Tooling" tab.
//!
//! Per-step data (operations, bindings, drill config) lives only in the datastore
//! document — the in-memory `JobProfile` projection is step-0 flattened — so this
//! reads `/steps/{i}/…` directly via [`with_appdata`], resolves the CNC/toolset, maps
//! the board holes to the assigner's `HoleDemand`, and calls `assign()`.
//!
//! Bed-safety: the fixture now carries the board-to-bed geometry, so both halves of
//! Z-feasibility are live — see [`build_setup`], shared with the Machining plan so the
//! two views agree. A step with no resolvable fixture relaxes the bed check (reach is
//! always enforced).
//!
//! Routing tools come from [`plan_routers`]: the board outline takes the smallest
//! available cutter, while each slot takes the largest that *fits it* — a cutter wider
//! than a slot cannot enter it, so the outline's choice is no guide. The same plan feeds
//! the Machining view's rack, keeping slot numbers identical across the two views.
//!
//! Scope note: outline router selection is still a heuristic pick of an available
//! router (size aside); it firms up when the geometry pre-pass lands.

use uuid::Uuid;

use datastore::{Node, NodeValue, UnitValue};
use units::user_format as unit_format;
use units::{FeedRate, Length, RotationalSpeed, UserUnitDisplay, UserUnitSystem};

use crate::data::model::tool_core::ToolKind;
use crate::data::model::{Tool, ToolsetGenerationPolicy};
use crate::data::{appdata_ready, with_appdata};
use crate::gcode::assigner::{
    self, Allowance, AssignConfig, AssignError, DemandKind, DepthDetail, FaultReason, HoleDemand,
    OverflowPolicy, RackSpec, Setup, Strategy, ToolAssignment, Weights,
};
use crate::gcode::feeds::{self, Limited, MachineLimits, RatedFeeds, SpindleRange};
use crate::gcode::step_data::StepValue;
use crate::runtime::AppState;

/// The full tooling plan: one entry per machining step (in order).
pub struct ToolingPlan {
    pub steps: Vec<StepPlan>,
    /// A top-level note when there is nothing to plan (no profile / no board).
    pub note: Option<String>,
    /// The rack schedules driving the Rack view — **one per rack the job uses**, not one
    /// per job. See [`RackSchedule`]. Empty when no step runs on a machine with a rack.
    pub racks: Vec<RackSchedule>,
}

impl ToolingPlan {
    /// The rack the step at `step_index` runs against, as it stands for that step.
    ///
    /// `None` when the step has no rack to describe: it resolved nothing, it loads no
    /// tools, or its machine has no tool changer at all. Searching every rack rather than
    /// indexing one is what makes a two-machine job work — a step belongs to exactly one
    /// rack, and which one is a fact about its CNC, not about its position in the job.
    pub fn rack_for_step(&self, step_index: usize) -> Option<RackStepView> {
        self.racks
            .iter()
            .find_map(|rack| rack.step_view(step_index))
    }
}

/// One physical rack across the job: its slots × the steps that run against it, each cell
/// the tool loaded and whether that slot must change before that step.
///
/// **A rack belongs to one CNC**, so a job whose steps run on two machines has two of
/// these. That is not a display nicety: [`SlotChange::Kept`] means "already loaded, do
/// nothing", and a tool left in one machine's carousel is not in another machine's. The
/// same is true of the toolset — re-racking a machine to a different toolset is a
/// different rack — so a schedule is keyed by the `(cnc, toolset)` pair its steps share.
/// Two CNC profiles may well be the same physical machine set up for different work; they
/// are still two racks here, because the app cannot know they are one.
///
/// Computed across every step of the rack even though the view shows one, because
/// `Kept` is only knowable from the sequence — "already in the rack" is a statement about
/// the steps before this one. [`Self::step_view`] projects one step out.
pub struct RackSchedule {
    /// The machine whose rack this is, for the operator: `T3 holds the 0.8 drill` says
    /// nothing in a job that runs on two machines unless it also says whose `T3`.
    pub cnc_name: String,
    /// Usable toolset slots this machine does not physically have — slots numbered past
    /// its ATC count. A toolset is a logical rack that may be a superset of any one
    /// machine's carousel (Specification §11.4), so this is a fact to report, not an
    /// error; those slots simply cannot be loaded here.
    pub clipped_slots: usize,
    /// One per *resolved* step that loads tools against this rack, in order — a
    /// **subset** of the profile's steps, so a column's position is not its step index.
    /// Hence [`RackStepColumn::step_index`].
    pub steps: Vec<RackStepColumn>,
    /// One row per physical (non-disabled, machine-reachable) slot, in `T`-order.
    pub slots: Vec<RackSlotSchedule>,
}

/// Which step a schedule column belongs to. Identity only — what the step is *called*
/// is the step chips' business.
pub struct RackStepColumn {
    /// Index into the profile's steps — *not* the column's own position.
    pub step_index: usize,
    /// Tools the step needs that the rack could not hold, already labelled. Empty in the
    /// ordinary case; non-empty means every slot was taken by a tool the step also needs,
    /// which the operator has to resolve at the machine.
    pub unplaced: Vec<String>,
}

/// One rack as it stands for a single step — what [`RackSchedule::step_view`] projects.
pub struct RackStepView {
    /// The machine this rack belongs to (see [`RackSchedule::cnc_name`]).
    pub cnc_name: String,
    /// How many of the job's steps run against this rack. The change status only says
    /// something once a step could have left a tool loaded for another.
    pub step_count: usize,
    pub slots: Vec<RackSlotView>,
    /// See [`RackStepColumn::unplaced`].
    pub unplaced: Vec<String>,
    /// See [`RackSchedule::clipped_slots`].
    pub clipped_slots: usize,
}

/// One slot as it stands for a single step.
#[derive(Clone, Debug, PartialEq)]
pub struct RackSlotView {
    /// Slot label, e.g. `T1`.
    pub slot: String,
    /// The tool in it for this step, or `None` when the slot is empty.
    pub tool: Option<String>,
    pub status: SlotChange,
}

pub struct RackSlotSchedule {
    /// Slot label, e.g. `T1`.
    pub slot: String,
    /// One cell per step, aligned with [`RackSchedule::steps`].
    pub cells: Vec<RackCell>,
}

pub struct RackCell {
    /// The tool in this slot for this step, or `None` when the slot is empty.
    pub tool: Option<String>,
    pub status: SlotChange,
}

/// Whether a slot must be changed before a step — the Rack view's colour code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotChange {
    /// Toolset-pinned: present from the start, never changes.
    Fixed,
    /// The operator must load or swap this tool into the slot before this step.
    Load,
    /// Carried over from the previous step in the same slot — no action.
    Kept,
    /// Unused this step.
    Empty,
}

pub struct StepPlan {
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
    /// Distinct tool ids the step loads, in slot order — the input the cross-step rack
    /// scheduler needs (the display rows carry labels, not ids).
    pub(crate) tool_ids: Vec<String>,
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
    pub(crate) fixture_id: Option<Uuid>,
    pub(crate) toolset_id: Option<Uuid>,
    pub(crate) drill: DrillConfigRaw,
    pub(crate) route_board: EdgeConfigRaw,
    pub(crate) route_cutouts: CutoutConfigRaw,
    pub(crate) engrave_copper: EngraveConfigRaw,
    /// `board_face == "back"`. Read per step rather than taken from the profile
    /// projection (which only carries `steps[0]`), because a later step may be the
    /// back-face one — which is exactly the step whose geometry the placement has to
    /// mirror.
    pub(crate) machines_back: bool,
    /// The registration pin this step drills for, when it drills locating pins at all.
    ///
    /// `None` means the step does not drill pins — *not* "drills pins of unknown size".
    /// The distinction matters: the job's coordinate frame grows around the pins, and
    /// only a step that actually has them may grow it.
    pub(crate) pin_diameter: Option<Length>,
}

/// What a step machines, asked of its enabled `operations`.
///
/// These read one line each, and existed as identical closures at the top of *both*
/// `plan_step`s (this module's and the operation planner's) plus, once the Board view
/// started drawing per step, a third caller. Three copies of "which key means drilling"
/// is three places for a renamed operation key to hide, so they live on the raw step
/// itself — the thing that actually owns the list.
impl StepRaw {
    /// Plated holes: pads and vias.
    pub(crate) fn drills_pth(&self) -> bool {
        self.enabled("drill_pth")
    }

    /// Non-plated holes: mounting holes and the like.
    pub(crate) fn drills_npth(&self) -> bool {
        self.enabled("drill_npth")
    }

    /// The board boundary. One operation, whose `outline.cut` says how it is made —
    /// `mill_board` was a second key for the same work and folds into this one on load
    /// (`fold_mill_board` in src/data/mod.rs).
    pub(crate) fn routes_outline(&self) -> bool {
        self.enabled("route_board")
    }

    /// The interior openings, cut on their own terms rather than with the edge kerf.
    pub(crate) fn routes_cutouts(&self) -> bool {
        self.enabled("route_cutouts")
    }

    /// Isolation engraving: a channel cut round every net so they come apart.
    pub(crate) fn engraves_copper(&self) -> bool {
        self.enabled("engrave_copper")
    }

    /// Registration pins: two holes on the fixture's flip line, drilled through the
    /// board and on into the backboard so it can be turned over and land back where it
    /// was. The one operation whose geometry comes from the *fixture* rather than from
    /// the board — KiCad has nothing to say about it.
    pub(crate) fn drills_locating_pins(&self) -> bool {
        self.enabled("drill_locating_pins")
    }

    /// How this step makes an oblong hole (drill the ends, route the slot, or both).
    pub(crate) fn oblong_strategy(&self) -> OblongStrategy {
        OblongStrategy::from_key(&self.drill.oblong)
    }

    fn enabled(&self, key: &str) -> bool {
        self.operations.iter().any(|op| op == key)
    }
}

/// How a through-cut contour is held until the operator breaks it out
/// (`route_board.*.retention`).
#[derive(Clone, Copy)]
pub(crate) struct RetentionRaw {
    /// `none` or `tabs`.
    pub(crate) tabs: bool,
    /// How many to place when the job positions none itself.
    pub(crate) count: usize,
    /// Length of contour each tab leaves uncut.
    pub(crate) width: Length,
    /// Perforate each tab so it snaps cleanly. A property *of* a tab, not an
    /// alternative to one.
    pub(crate) mouse_bites: bool,
}

impl RetentionRaw {
    /// The schema's defaults for the board's boundary — four tabs, one a side.
    ///
    /// It used to take a `count`, because a cutout's slug wanted one or two rather than
    /// four. Slugs are `route_cutouts`' business now and it holds them its own way, so
    /// there is only ever one answer left to give.
    fn defaults() -> Self {
        Self {
            tabs: true,
            count: 4,
            width: Length::from_mm(2.0),
            mouse_bites: false,
        }
    }
}

/// The step's `route_board` config — the board-routing **policy**, defaulted when absent.
///
/// Only the policy: how the boundary is cut and how it is retained. Where the tabs
/// actually sit is not here and cannot be — a machining profile is reused across boards,
/// and a tab position means nothing without one specific outline. That lives on the job
/// (`job.yaml#/edge_tabs`).
///
/// **The boundary and nothing else.** There were `cutouts` and `cutout_retention` fields
/// here, and the outline pass cut the interior openings with the edge kerf. Both are gone
/// with the schema block behind them: `route_cutouts` owns the openings now.
pub(crate) struct EdgeConfigRaw {
    /// How the boundary is made: `route | mill | score | vgroove`.
    pub(crate) cut: String,
    /// Retention for the board's own boundary.
    pub(crate) outline: RetentionRaw,
    /// Width of the channel routed around the board — and so, exactly, the diameter of
    /// the cutter that routes it.
    pub(crate) kerf: Length,
    /// Material left on the wall for a finishing pass; zero means none.
    pub(crate) finishing: Length,
}

impl EdgeConfigRaw {
    /// Whether the boundary is cut right through by a router — the only mode the outline
    /// phase plans today. Scoring and V-grooving cut partway and need a depth model and
    /// a V-bit, which the tool stock does not carry yet.
    pub(crate) fn cuts_through(&self) -> bool {
        matches!(self.cut.as_str(), "route" | "mill")
    }
}

impl Default for EdgeConfigRaw {
    fn default() -> Self {
        // The schema's own defaults for `route_board`.
        Self {
            cut: "route".to_string(),
            outline: RetentionRaw::defaults(),
            kerf: Length::from_mm(2.0),
            finishing: Length::from_mm(0.1),
        }
    }
}

/// The step's `route_cutouts` config, defaulted when absent.
///
/// No kerf and no retention *mode*, unlike the edge: the cutter is chosen by fit rather
/// than requested, and a slug is either held or it is a projectile.
pub(crate) struct CutoutConfigRaw {
    pub(crate) retain_island: bool,
    /// Tab width as a fraction of the slug perimeter — `"4%"` in the schema, `0.04` here.
    pub(crate) tab_ratio: f64,
    pub(crate) drill_sharp_corners: bool,
}

impl Default for CutoutConfigRaw {
    fn default() -> Self {
        // The schema's own defaults for `route_cutouts`.
        Self {
            retain_island: true,
            tab_ratio: 0.04,
            drill_sharp_corners: true,
        }
    }
}

fn read_cutout_config(root: &Node, base: &str) -> CutoutConfigRaw {
    let default = CutoutConfigRaw::default();
    CutoutConfigRaw {
        retain_island: node_bool(root, &format!("{base}/retain_island"))
            .unwrap_or(default.retain_island),
        tab_ratio: node_percent_fraction(root, &format!("{base}/island_tab"))
            .unwrap_or(default.tab_ratio),
        drill_sharp_corners: node_bool(root, &format!("{base}/drill_sharp_corners"))
            .unwrap_or(default.drill_sharp_corners),
    }
}

/// The step's `engrave_copper` config, defaulted when absent.
///
/// One setting, and depth is deliberately not among them: a V-bit cuts as wide as it is
/// deep, so depth follows from this width and whichever bit the rack can offer.
pub(crate) struct EngraveConfigRaw {
    pub(crate) width: Length,
}

impl Default for EngraveConfigRaw {
    fn default() -> Self {
        // The schema's own default for `engrave_copper`.
        Self { width: Length::from_mm(0.25) }
    }
}

fn read_engrave_config(root: &Node, base: &str) -> EngraveConfigRaw {
    let default = EngraveConfigRaw::default();
    EngraveConfigRaw {
        width: node_length(root, &format!("{base}/width")).unwrap_or(default.width),
    }
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

/// How a step makes an oblong hole — the schema's `holes.oblong` enum, resolved once so
/// the Tooling tab and the Machining plan cannot read the same key differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OblongStrategy {
    /// Mill the whole slot with a router.
    Route,
    /// Drill the two end centres, then mill the web between them.
    DrillEndsThenRoute,
    /// A chain of overlapping drills — the scallops between them are the finished wall.
    DrillChain,
    /// A chain of overlapping drills, then a router cleanup pass on the walls.
    DrillChainThenRoute,
}

impl OblongStrategy {
    /// Parses the schema key, falling back to the schema's own default so an unknown or
    /// missing value machines conservatively rather than not at all.
    pub(crate) fn from_key(key: &str) -> Self {
        match key {
            "route" => Self::Route,
            "drill_chain" => Self::DrillChain,
            "drill_chain_then_route" => Self::DrillChainThenRoute,
            _ => Self::DrillEndsThenRoute,
        }
    }

    /// Whether the strategy puts a drill in the slot at all.
    pub(crate) fn drills(self) -> bool {
        !matches!(self, Self::Route)
    }

    /// Whether the strategy needs a router — and so a cutter that fits the slot width.
    pub(crate) fn routes(self) -> bool {
        !matches!(self, Self::DrillChain)
    }

    /// Whether the router meets full material. False only for `drill_chain_then_route`,
    /// where the chain has already opened the channel and all that is left is the
    /// finishing lap on the wall.
    pub(crate) fn routes_from_solid(self) -> bool {
        !matches!(self, Self::DrillChainThenRoute)
    }

    /// The chain's pitch ceiling as a fraction of the drill diameter, or `None` when the
    /// strategy drills only the two end centres. Callers gate on [`Self::drills`] first.
    pub(crate) fn chain_pitch_fraction(self) -> Option<f64> {
        match self {
            // The chain is the finished wall, so the scallops must stay small.
            Self::DrillChain => Some(crate::gcode::oblong::CHAIN_PITCH_FINISH),
            // A router cleans up after, so the chain only has to remove bulk.
            Self::DrillChainThenRoute => Some(crate::gcode::oblong::CHAIN_PITCH_ROUGH),
            // Ends only — the web between them is the router's job.
            Self::Route | Self::DrillEndsThenRoute => None,
        }
    }
}

impl Default for DrillConfigRaw {
    fn default() -> Self {
        Self {
            route_fallback: false,
            drill_first: true,
            pilot: false,
            oblong: "drill_ends_then_route".to_string(),
            oversize: Allowance {
                relative: 0.08,
                max: Length::from_mm(0.10),
            },
            undersize: Allowance {
                relative: 0.06,
                max: Length::from_mm(0.08),
            },
        }
    }
}

/// Builds the tooling plan for the current context. Reads all steps of the selected
/// machining profile, runs the assigner for each, and formats the outcome.
pub fn plan_tooling(ctx: &AppState, stitched: Option<&pcb::StitchResult>) -> ToolingPlan {
    let Some(profile_id) = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return ToolingPlan {
            steps: vec![],
            note: Some("Select a machining profile to plan tooling.".into()),
            racks: vec![],
        };
    };
    if ctx.board.is_none() {
        return ToolingPlan {
            steps: vec![],
            note: Some("No board loaded — nothing to machine.".into()),
            racks: vec![],
        };
    }
    if !appdata_ready() {
        return ToolingPlan {
            steps: vec![],
            note: Some("Configuration store is not ready.".into()),
            racks: vec![],
        };
    }

    let raw_steps = read_steps(profile_id);
    if raw_steps.is_empty() {
        return ToolingPlan {
            steps: vec![],
            note: Some("The machining profile has no steps.".into()),
            racks: vec![],
        };
    }

    // File each resolved step under the rack it runs against as the steps are planned;
    // the schedules are built once they are all known, because what a slot holds for one
    // step depends on the steps before it *on that machine*.
    let mut groups: Vec<RackGroup> = Vec::new();

    let steps: Vec<StepPlan> = raw_steps
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let outcome = plan_step(ctx, stitched, &raw);
            if let StepOutcome::Resolved(resolved) = &outcome {
                collect_rack_step(ctx, &mut groups, index, &raw, &resolved.tool_ids);
            }
            StepPlan {
                name: raw.name.clone(),
                outcome,
            }
        })
        .collect();

    let racks = groups
        .iter()
        .filter_map(|group| {
            let machine = ctx.machines.iter().find(|m| m.id == group.cnc_id)?;
            let toolset = ctx.toolsets.iter().find(|t| t.id == group.toolset_id)?;
            Some(build_rack_schedule(ctx, machine, toolset, &group.steps))
        })
        .collect();

    ToolingPlan {
        steps,
        note: None,
        racks,
    }
}

/// The steps sharing one physical rack, gathered while the job is planned.
struct RackGroup {
    /// The `(cnc, toolset)` pair that identifies the rack — see [`RackSchedule`]. Held as
    /// the profile ids so the group stays a key rather than a copy of two profiles.
    cnc_id: String,
    toolset_id: String,
    steps: Vec<RackStepInput>,
}

/// One step's contribution to a rack schedule.
struct RackStepInput {
    /// Index into the profile's steps.
    step_index: usize,
    /// The distinct tools the step loads, in slot order.
    tool_ids: Vec<String>,
}

/// Files a resolved step under the rack it runs against, opening a group the first time
/// that `(cnc, toolset)` pair is seen.
///
/// Two kinds of step contribute nothing. One that loads no tools has no rack state to
/// describe. One whose machine has **no tool changer** has no rack at all: its tools go
/// into the spindle one at a time, so "kept from the last step" is not a thing that can
/// happen to it — the Tooling tab's T-order is its change sequence, and that is the whole
/// truth about its tooling.
///
/// Grouping is by identity rather than by adjacency: steps 1 and 3 on the same machine
/// share its rack even with a step on another machine between them, because nothing
/// touched that machine in the meantime.
fn collect_rack_step(
    ctx: &AppState,
    groups: &mut Vec<RackGroup>,
    step_index: usize,
    raw: &StepRaw,
    tool_ids: &[String],
) {
    if tool_ids.is_empty() {
        return;
    }
    let (Some(cnc_id), Some(toolset_id)) = (raw.cnc_id, raw.toolset_id) else {
        return;
    };
    let (cnc_id, toolset_id) = (cnc_id.to_string(), toolset_id.to_string());
    let has_rack = ctx
        .machines
        .iter()
        .any(|machine| machine.id == cnc_id && machine.atc_slot_count > 0);
    if !has_rack {
        return;
    }

    let step = RackStepInput {
        step_index,
        tool_ids: tool_ids.to_vec(),
    };
    match groups
        .iter_mut()
        .find(|group| group.cnc_id == cnc_id && group.toolset_id == toolset_id)
    {
        Some(group) => group.steps.push(step),
        None => groups.push(RackGroup {
            cnc_id,
            toolset_id,
            steps: vec![step],
        }),
    }
}

/// Builds one machine's cross-step rack schedule: each physical slot's tool per step,
/// with the change status. A dynamic tool keeps a **stable slot** — loaded once (`Load`)
/// and `Kept` afterwards — so inter-step changes are minimised. Empty spare slots are
/// used before any tool is evicted; only when every spare slot holds a still-needed tool
/// is one reused (that reload shows as `Load`). Fixed (toolset-pinned) slots never
/// change.
///
/// `steps` are the steps that run against *this* rack, which is why the machine is a
/// parameter beside the toolset: the same toolset loaded into a 8-slot carousel and a
/// 12-slot one is two different racks, and only the machine knows which slots exist.
fn build_rack_schedule(
    ctx: &AppState,
    machine: &crate::data::model::MachineProfile,
    toolset: &crate::data::model::ToolsetProfile,
    steps: &[RackStepInput],
) -> RackSchedule {
    use std::collections::{BTreeMap, BTreeSet};

    // Classify the physical (non-disabled) slots into fixed (pinned tool) and spare,
    // dropping the ones this machine's carousel does not reach. Slots are `T1`-based, so
    // a machine with N of them has T1..=TN; a toolset may legitimately define more
    // (Specification §11.4) and those simply cannot be loaded here.
    let mut fixed: BTreeMap<u8, String> = BTreeMap::new();
    let mut reserved: Vec<u8> = Vec::new();
    let mut spare_slots: Vec<u8> = Vec::new();
    let mut clipped_slots = 0usize;
    for (index, slot) in toolset.slots.iter() {
        if slot.disabled {
            continue;
        }
        if *index > machine.atc_slot_count {
            clipped_slots += 1;
            continue;
        }
        match (slot.locked, slot.tool_id.as_ref()) {
            (true, Some(tool)) => {
                fixed.insert(*index, tool.clone());
            }
            (true, None) => reserved.push(*index), // pinned to nothing: shown, never filled
            (false, _) => spare_slots.push(*index),
        }
    }
    spare_slots.sort_unstable();
    let fixed_tools: BTreeSet<String> = fixed.values().cloned().collect();

    let snapshots = schedule_spare_slots(&spare_slots, &fixed_tools, steps);

    // Build one row per physical slot (fixed first, then spare — both in index order).
    // A locked slot holding nothing is reserved: the assigner never fills it, so it gets
    // a fixed, empty row rather than disappearing from the rack the operator is reading.
    let mut rows: Vec<RackSlotSchedule> = Vec::new();
    let mut all_slots: Vec<(u8, bool)> = reserved
        .iter()
        .chain(fixed.keys())
        .map(|s| (*s, true))
        .chain(spare_slots.iter().map(|s| (*s, false)))
        .collect();
    all_slots.sort_by_key(|(index, _)| *index);

    for (index, is_fixed) in all_slots {
        let cells: Vec<RackCell> = snapshots
            .iter()
            .map(|snapshot| {
                if is_fixed {
                    RackCell {
                        tool: fixed.get(&index).map(|id| tool_label(ctx, id)),
                        status: SlotChange::Fixed,
                    }
                } else {
                    match snapshot.state.get(&index) {
                        Some(id) => RackCell {
                            tool: Some(tool_label(ctx, id)),
                            status: if snapshot.changed.contains(&index) {
                                SlotChange::Load
                            } else {
                                SlotChange::Kept
                            },
                        },
                        None => RackCell {
                            tool: None,
                            status: SlotChange::Empty,
                        },
                    }
                }
            })
            .collect();
        rows.push(RackSlotSchedule {
            slot: format!("T{index}"),
            cells,
        });
    }

    RackSchedule {
        cnc_name: machine.name.clone(),
        clipped_slots,
        steps: steps
            .iter()
            .zip(snapshots.iter())
            .map(|(step, snapshot)| RackStepColumn {
                step_index: step.step_index,
                unplaced: snapshot
                    .unplaced
                    .iter()
                    .map(|id| tool_label(ctx, id))
                    .collect(),
            })
            .collect(),
        slots: rows,
    }
}

impl RackSchedule {
    /// This rack as it stands **for one step**: every physical slot, the tool in it, and
    /// whether the operator must change that slot before running this step.
    ///
    /// `None` when the step has no column here — it runs against a different rack, or
    /// none at all. Projected rather than recomputed because [`SlotChange::Kept`] depends
    /// on the steps before this one; the schedule is the only place that knows.
    pub fn step_view(&self, step_index: usize) -> Option<RackStepView> {
        // Column position is not step index — only the resolved, tool-loading steps that
        // run against *this* machine get columns.
        let column = self.steps.iter().position(|s| s.step_index == step_index)?;
        Some(RackStepView {
            cnc_name: self.cnc_name.clone(),
            step_count: self.steps.len(),
            slots: self
                .slots
                .iter()
                .map(|row| RackSlotView {
                    slot: row.slot.clone(),
                    tool: row.cells.get(column).and_then(|cell| cell.tool.clone()),
                    status: row
                        .cells
                        .get(column)
                        .map(|cell| cell.status)
                        .unwrap_or(SlotChange::Empty),
                })
                .collect(),
            unplaced: self.steps[column].unplaced.clone(),
            clipped_slots: self.clipped_slots,
        })
    }
}

/// One step's spare-slot state in the cross-step schedule.
struct SpareSnapshot {
    /// Which tool sits in which spare slot once this step's changes are made.
    state: std::collections::BTreeMap<u8, String>,
    /// The slots the operator must change before this step.
    changed: std::collections::BTreeSet<u8>,
    /// Tools the step needs that no slot could take, because every spare slot already
    /// holds a tool the same step needs. The rack is over capacity for this step; the
    /// tools are named rather than dropped, so the view can say so.
    unplaced: Vec<String>,
}

/// The core cross-step spare-slot schedule (ctx-free, so it is unit-testable): one
/// [`SpareSnapshot`] per step, in step order. A tool already loaded is kept in place; a
/// new one takes an empty slot, or evicts a not-needed one. Fixed tools are excluded
/// (they never occupy a spare slot).
fn schedule_spare_slots(
    spare_slots: &[u8],
    fixed_tools: &std::collections::BTreeSet<String>,
    steps: &[RackStepInput],
) -> Vec<SpareSnapshot> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut loaded: BTreeMap<u8, String> = BTreeMap::new();
    let mut snapshots = Vec::new();
    for step in steps {
        let dynamic: Vec<&String> = step
            .tool_ids
            .iter()
            .filter(|t| !fixed_tools.contains(*t))
            .collect();
        let needed: BTreeSet<&String> = dynamic.iter().copied().collect();
        let mut changed: BTreeSet<u8> = BTreeSet::new();
        let mut unplaced: Vec<String> = Vec::new();
        for tool in dynamic {
            if loaded.values().any(|t| t == tool) {
                continue; // already in the rack — kept
            }
            match pick_slot(spare_slots, &loaded, &needed) {
                Some(slot) => {
                    loaded.insert(slot, tool.clone());
                    changed.insert(slot);
                }
                None => unplaced.push(tool.clone()),
            }
        }
        snapshots.push(SpareSnapshot {
            state: loaded.clone(),
            changed,
            unplaced,
        });
    }
    snapshots
}

/// Picks a spare slot for a tool that must be loaded: an empty slot first (so nothing
/// is disturbed), else a slot whose current tool is not needed this step (safe to
/// evict). `None` when every spare slot holds a still-needed tool (capacity overflow).
fn pick_slot(
    spare_slots: &[u8],
    loaded: &std::collections::BTreeMap<u8, String>,
    needed: &std::collections::BTreeSet<&String>,
) -> Option<u8> {
    if let Some(&empty) = spare_slots.iter().find(|s| !loaded.contains_key(s)) {
        return Some(empty);
    }
    spare_slots
        .iter()
        .find(|s| loaded.get(s).map(|t| !needed.contains(t)).unwrap_or(false))
        .copied()
}

/// Reads every step's operations, bindings and drill config from the profile document.
/// What the selected step actually machines, as the Board view needs it.
///
/// Derived from the very predicates the Tooling and Machining plans use, so the drawing
/// cannot claim a feature is machined that the plan skips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StepTargets {
    pub pth: bool,
    pub npth: bool,
    /// The board boundary — the outline band is the step's work only if it routes or
    /// mills it.
    pub outline: bool,
}

impl StepTargets {
    /// Whether this step makes `hole`.
    ///
    /// Delegated to [`HoleGroup::from_hole`], which is already the app's single hole
    /// classifier, rather than re-deciding here: one answer, not two that could drift.
    /// The oblong strategy needs no separate condition — every strategy either drills the
    /// ends or routes the slot, so a slot this step's operations cover is machined either
    /// way.
    pub fn machines(&self, hole: &pcb::BoardHole) -> bool {
        HoleGroup::from_hole(hole, self.pth, self.npth).is_some()
    }
}

/// What the step at `index` machines, or `None` when there is no such step to ask about
/// — in which case the Board view draws the whole board, which is what a board with no
/// machining profile selected should look like.
pub fn step_targets(ctx: &AppState, index: usize) -> Option<StepTargets> {
    let profile_id = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())?;
    if !appdata_ready() {
        return None;
    }
    let raw = read_steps(profile_id).into_iter().nth(index)?;
    Some(StepTargets {
        pth: raw.drills_pth(),
        npth: raw.drills_npth(),
        outline: raw.routes_outline(),
    })
}

/// One machining step as the Job chrome needs it: what to call it, and the couple of
/// facts that decide what is shown for it.
#[derive(Clone, Debug, PartialEq)]
pub struct StepHeader {
    pub index: usize,
    pub name: String,
    pub cnc_name: String,
    /// Whether *this step's* CNC changes tools automatically. Per step because the Rack
    /// tab is only meaningful for a machine with a rack, and two steps of one job may run
    /// on different machines.
    pub is_atc: bool,
}

/// The selected profile's steps, in order — the one read the step chips, the Rack tab
/// gate and the save dialog share.
///
/// Read from the datastore rather than derived from a plan: a plan yields no steps when
/// no board is loaded or the store is not ready, and the step chrome must not blink out
/// of existence because KiCad went away.
pub fn step_headers(ctx: &AppState) -> Vec<StepHeader> {
    let Some(profile_id) = ctx
        .selected_process_profile_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return Vec::new();
    };
    if !appdata_ready() {
        return Vec::new();
    }
    read_steps(profile_id)
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            let machine = raw
                .cnc_id
                .and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()));
            StepHeader {
                index,
                name: raw.name,
                cnc_name: machine.map(|m| m.name.clone()).unwrap_or_default(),
                is_atc: machine.map(|m| m.atc_slot_count > 0).unwrap_or(false),
            }
        })
        .collect()
}

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
                let route_board = read_edge_config(root, &format!("/steps/{i}/route_board"));
                let route_cutouts = read_cutout_config(root, &format!("/steps/{i}/route_cutouts"));
                let engrave_copper =
                    read_engrave_config(root, &format!("/steps/{i}/engrave_copper"));

                // Only when the step actually drills pins. The per-operation config
                // objects are materialised by the loader whether or not their operation
                // is enabled, so reading this unconditionally would give every step a pin
                // diameter — and the job's coordinate frame would then grow around pins
                // that nothing drills.
                let pin_diameter = operations
                    .iter()
                    .any(|op| op == "drill_locating_pins")
                    .then(|| {
                        node_pin_diameter(
                            root,
                            &format!("/steps/{i}/drill_locating_pins/pin_diameter"),
                        )
                    })
                    .flatten();

                StepRaw {
                    // The name as it is *shown*, not as it is stored: a step the operator
                    // has not named is called after what it does. Resolved here, once, so
                    // the step chips, the plan's notes, the readiness messages and the
                    // saved file names all agree — see `step_display_name`.
                    name: crate::data::model::step_display_name(
                        &node_str(root, &format!("/steps/{i}/name")).unwrap_or_default(),
                        &operations,
                    ),
                    operations,
                    cnc_id: node_ref(root, &format!("/steps/{i}/cnc")),
                    fixture_id: node_ref(root, &format!("/steps/{i}/fixture")),
                    toolset_id: node_ref(root, &format!("/steps/{i}/toolset")),
                    drill,
                    route_board,
                    route_cutouts,
                    engrave_copper,
                    machines_back: node_str(root, &format!("/steps/{i}/board_face"))
                        .is_some_and(|face| face.eq_ignore_ascii_case("back")),
                    pin_diameter,
                }
            })
            .collect()
    })
}

/// Reads the profile's `steps` array whole, as the program templates see it.
///
/// The complement of [`read_steps`]: that one projects the handful of fields the planner
/// acts on, hand-written field by field. This one takes the array verbatim, so a field
/// added to `machining.yaml`'s `$defs/step` reaches `program_begin` without a change
/// here — the templates are the operator's, and which of their own settings they want to
/// print in a header is not this application's to enumerate.
///
/// Walks [`NodeValue`] rather than calling `Node::to_value()`, because that renders a
/// `2.0mm` as the string `"2.0mm"`. Keeping it a [`Length`] is what lets a template
/// interpolate it and have it follow the program's own unit mode — see
/// [`StepValue`](crate::gcode::step_data::StepValue).
pub(crate) fn read_step_values(profile_id: Uuid) -> Vec<StepValue> {
    with_appdata(|data| {
        let Some(doc) = data.get(profile_id) else {
            return Vec::new();
        };
        match doc.root.get_pointer("/steps").map(|node| &node.value) {
            Some(NodeValue::Array(items)) => items.iter().map(node_to_step_value).collect(),
            _ => Vec::new(),
        }
    })
}

/// One document node as the template-facing tree.
///
/// An identity and a reference both become their UUID text: a template that wants to
/// name the machine a step binds uses the `cnc_name` the orchestrator resolves beside
/// it, and the raw id stays available for one that needs it verbatim.
fn node_to_step_value(node: &Node) -> StepValue {
    match &node.value {
        NodeValue::Null => StepValue::Null,
        NodeValue::Bool(v) => StepValue::Bool(*v),
        NodeValue::Int(v) => StepValue::Int(*v),
        NodeValue::Float(v) => StepValue::Number(*v),
        NodeValue::Str(v) => StepValue::Text(v.clone()),
        NodeValue::Id(id) => StepValue::Text(id.to_string()),
        NodeValue::Ref(r) => StepValue::Text(r.raw.to_string()),
        NodeValue::Unit(UnitValue::Length(v)) => StepValue::Length(*v),
        NodeValue::Unit(UnitValue::Feed(v)) => StepValue::Feed(*v),
        NodeValue::Unit(UnitValue::Angle(v)) => StepValue::Angle(*v),
        NodeValue::Unit(UnitValue::Rpm(v)) => StepValue::Rpm(*v),
        NodeValue::Array(items) => StepValue::List(items.iter().map(node_to_step_value).collect()),
        NodeValue::Object(fields) => StepValue::Map(
            fields
                .iter()
                .map(|(key, child)| (key.clone(), node_to_step_value(child)))
                .collect(),
        ),
    }
}

/// Reads a `retention` block at `base`, falling back to `default` per field.
fn read_retention(root: &Node, base: &str, default: RetentionRaw) -> RetentionRaw {
    RetentionRaw {
        tabs: node_str(root, &format!("{base}/mode"))
            .map(|mode| mode != "none")
            .unwrap_or(default.tabs),
        count: node_count(root, &format!("{base}/count")).unwrap_or(default.count),
        width: node_length(root, &format!("{base}/width")).unwrap_or(default.width),
        mouse_bites: node_bool(root, &format!("{base}/mouse_bites")).unwrap_or(default.mouse_bites),
    }
}

/// Reads the `route_board` config at `base`, falling back to defaults per field.
fn read_edge_config(root: &Node, base: &str) -> EdgeConfigRaw {
    let default = EdgeConfigRaw::default();
    EdgeConfigRaw {
        cut: node_str(root, &format!("{base}/outline/cut")).unwrap_or(default.cut),
        outline: read_retention(root, &format!("{base}/outline/retention"), default.outline),
        kerf: node_length(root, &format!("{base}/kerf")).unwrap_or(default.kerf),
        finishing: node_length(root, &format!("{base}/finishing")).unwrap_or(default.finishing),
    }
}

/// Reads the `holes` drill config at `base`, falling back to defaults per field.
fn read_drill_config(root: &Node, base: &str) -> DrillConfigRaw {
    let default = DrillConfigRaw::default();
    DrillConfigRaw {
        route_fallback: node_bool(root, &format!("{base}/route_fallback"))
            .unwrap_or(default.route_fallback),
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
        relative: node_percent_fraction(root, &format!("{base}/relative"))
            .unwrap_or(fallback.relative),
        max: node_length(root, &format!("{base}/max")).unwrap_or(fallback.max),
    }
}

/// Builds the assigner's Z-feasibility [`Setup`] from the board + the step's fixture.
///
/// Shared by the Tooling tab and the Machining plan so the two **agree** on which
/// tools are feasible (the same assignment must back both views). Board thickness
/// comes from the KiCad stackup; the below-board space the tool tip may use is the
/// martyr-board thickness minus the safety margin kept off the bed
/// (`backboard_thickness − bed_clearance`), and the breakthrough margin is the
/// fixture's `breakthrough`. With no fixture resolved the bed check is relaxed (so a
/// mid-configuration view still plans); the reach check is always enforced.
pub(crate) fn build_setup(ctx: &AppState, fixture_id: Option<Uuid>) -> Setup {
    let fixture = fixture_id.and_then(|id| ctx.fixtures.iter().find(|f| f.id == id.to_string()));
    Setup {
        board_thickness: ctx
            .board
            .as_ref()
            .and_then(|b| b.thickness)
            .unwrap_or(Length::from_mm(1.6)),
        bed_clearance: fixture
            .map(|f| {
                Length::from_mm((f.backboard_thickness.as_mm() - f.bed_clearance.as_mm()).max(0.0))
            })
            .unwrap_or(Length::from_mm(1_000.0)),
        breakthrough_margin: fixture
            .map(|f| f.breakthrough)
            .unwrap_or(Length::from_mm(0.5)),
    }
}

/// The operator-facing reason a step cannot be planned for want of a profile, or `None`
/// when all three bindings are set.
///
/// A step references exactly one CNC, fixture and toolset, and any of them may be unset —
/// which is a deliberate state (it is how a half-configured step reads), not something to
/// default away. Shared by the Tooling tab and the Machining plan so both refuse the same
/// steps with the same words.
pub(crate) fn missing_bindings(raw: &StepRaw) -> Option<String> {
    let missing: Vec<&str> = [
        ("CNC", raw.cnc_id.is_none()),
        ("fixture", raw.fixture_id.is_none()),
        ("toolset", raw.toolset_id.is_none()),
    ]
    .into_iter()
    .filter_map(|(label, absent)| absent.then_some(label))
    .collect();

    (!missing.is_empty()).then(|| {
        format!(
            "This step has no {} profile selected.",
            missing.join(", no ")
        )
    })
}

/// Everything wrong with how a profile arranges its locating-pin step, one operator-facing
/// sentence each. Empty when the arrangement is sound.
///
/// This replaced a blanket refusal of every back-face step, which existed only because
/// nothing mirrored the geometry. The mirror is real now, so what has to be checked is no
/// longer "is any step on the back" but "can the board actually get there":
///
/// 1. **Pins come first.** They are drilled into a board that is still in its original
///    setup; drilling them after something else has already cut the board means
///    registering it against holes made after the fact, which registers nothing.
/// 2. **Pins are a front-face operation.** Drilling registration from the back means the
///    board was already turned over — before it had anything to be turned over *against*.
/// 3. **A face change needs pins.** Two steps on opposite faces with no pins between them
///    is a board lifted off the fixture and put back by eye. The program that follows is
///    exact to a micron and lands wherever the operator happened to put it.
///
/// The last is the only one the machining editor cannot prevent (it hides the face control
/// on a pins step and pins the first slot), so the other two are reachable mainly through
/// a hand-edited or imported profile — which is precisely what a readiness gate is for.
///
/// Takes the steps rather than a profile id so the decision is separable from reading the
/// document — and so it can be tested, which the global-store path cannot be. Every step is
/// checked, not the profile's projected face: that projection carries `steps[0]` only, and
/// a back-face *second* step is exactly what it would miss.
pub(crate) fn locating_pin_faults(steps: &[StepRaw]) -> Vec<String> {
    let mut faults = Vec::new();

    let first_pins = steps.iter().position(StepRaw::drills_locating_pins);

    for (index, step) in steps.iter().enumerate() {
        if !step.drills_locating_pins() {
            continue;
        }
        if index > 0 {
            faults.push(format!(
                "{} drills locating pins but is not the first step. Registration has to be \
                 drilled before anything else cuts the board — move it to the top.",
                crate::data::model::step_reference(index, &step.name),
            ));
        }
        if step.machines_back {
            faults.push(format!(
                "{} drills locating pins on the back face. Pins are what lets the board be \
                 turned over, so they are drilled on the front — set its board face to front.",
                crate::data::model::step_reference(index, &step.name),
            ));
        }
    }

    // The first two steps that disagree about which face they machine. Only the first pair
    // is reported: they all share one cause, and listing every later step as well would
    // bury the fix.
    if let Some((a, b)) = first_face_change(steps) {
        // Before the step that turns the board over — not merely present. Pins drilled at
        // or after the change have not registered the board the earlier steps were cut in.
        // Strictly before `b`, and *not* before `a`: `a` is step 1, which the pins step
        // itself normally is, and requiring them to precede themselves would fault every
        // correctly-built profile.
        let registered = first_pins.is_some_and(|at| at < b);
        if !registered {
            faults.push(format!(
                "steps {} and {} machine different faces but no locating-pins step precedes \
                 them — add one first",
                a + 1,
                b + 1,
            ));
        }
    }

    faults
}

/// The first two steps that machine opposite faces, as `(earlier, later)` indexes.
///
/// The *first* step is one end of the pair rather than the immediately preceding step,
/// because the board is turned over once: a front/front/back profile changes face between
/// steps 1 and 3, and naming steps 2 and 3 would point at the wrong boundary.
fn first_face_change(steps: &[StepRaw]) -> Option<(usize, usize)> {
    let first = steps.first()?;
    steps
        .iter()
        .position(|step| step.machines_back != first.machines_back)
        .map(|at| (0, at))
}

/// Describes any operation two steps both claim on the same board face, or `None` when
/// every step's work is its own. The readiness gate turns this into a no-go reason.
///
/// The machining editor disables the checkbox that would create such a profile, so this
/// exists for the ones it never saw: hand-edited YAML, an imported profile, or one
/// written before the rule existed. Blocking rather than warning, because the output is
/// not degraded but wrong — two programs driving a tool through the same holes, the
/// second one into a board that the first has already released from its tabs.
///
/// Takes the steps rather than a profile id for the same reason
/// [`locating_pin_faults`] does: so the decision is testable without the store.
pub(crate) fn duplicate_operations_reason(steps: &[StepRaw]) -> Option<String> {
    let conflicts = crate::data::model::conflicting_operations(steps.iter().map(|step| {
        (
            step.name.as_str(),
            step.machines_back,
            step.operations.as_slice(),
        )
    }));

    (!conflicts.is_empty()).then(|| {
        conflicts
            .iter()
            .map(|conflict| conflict.message())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Plans one step: builds demands + rack, runs the assigner, formats the outcome.
fn plan_step(ctx: &AppState, stitched: Option<&pcb::StitchResult>, raw: &StepRaw) -> StepOutcome {
    let has_pth = raw.drills_pth();
    let has_npth = raw.drills_npth();
    let has_route = raw.routes_outline();
    let has_locating = raw.drills_locating_pins();

    // Every binding is required, and for the same reason the Machining plan requires
    // them: a defaulted fixture or CNC yields a plausible answer about hardware the
    // operator does not have. Both views must agree on which steps are plannable at all,
    // so this check is the same one, worded the same way.
    if let Some(reason) = missing_bindings(raw) {
        return StepOutcome::Failed(vec![reason]);
    }
    let Some(toolset) = raw
        .toolset_id
        .and_then(|id| ctx.toolsets.iter().find(|t| t.id == id.to_string()))
    else {
        return StepOutcome::Failed(vec!["The step's toolset profile could not be found.".into()]);
    };
    // The whole profile, not just its ATC count: the spindle range is needed further down
    // to say whether the step's tools can actually run at their rated speeds.
    let Some(machine) = raw
        .cnc_id
        .and_then(|id| ctx.machines.iter().find(|m| m.id == id.to_string()))
    else {
        return StepOutcome::Failed(vec!["The step's CNC profile could not be found.".into()]);
    };
    let atc_slots = machine.atc_slot_count as usize;

    // Build the hole demand set from the board, grouped by (kind, size) for counts.
    let holes = ctx
        .board
        .as_ref()
        .map(|b| b.holes.as_slice())
        .unwrap_or(&[]);
    let groups = collect_hole_groups(holes, has_pth, has_npth);

    // A router is needed for the board outline and/or for oblong slots that route.
    let has_oblongs = groups.iter().any(|g| g.minor.is_some());
    let oblong = raw.oblong_strategy();
    let (oblong_drills, oblong_routes) = (oblong.drills(), oblong.routes());
    let routes_slots = has_oblongs && oblong_routes;

    let mut warnings: Vec<String> = Vec::new();
    let cutout_contours = cutout_contours(stitched, raw);
    let routers = plan_routers(
        &ctx.tools,
        toolset,
        &groups,
        has_route,
        raw.route_board.kerf,
        routes_slots,
        CutoutRouting {
            contours: &cutout_contours,
            relieve_corners: raw.routes_cutouts() && raw.route_cutouts.drill_sharp_corners,
        },
    );
    for index in &routers.uncuttable_cutouts {
        warnings.push(uncuttable_cutout_warning(ctx, *index));
    }
    if has_route && routers.outline.is_none() {
        warnings.push(format!(
            "No {} router in stock, which is what the board's {} edge kerf needs. A kerf is              the cutter that makes it, so nothing else will cut it to size — stock that              cutter, or set the step's kerf to a size you have.",
            fmt_len(ctx, raw.route_board.kerf),
            fmt_len(ctx, raw.route_board.kerf),
        ));
    }
    for width in &routers.unroutable_widths {
        warnings.push(unroutable_slot_warning(ctx, *width));
    }
    // The locating-pin tool, resolved exactly as the Machining plan resolves it, so the
    // rack this tab shows is the rack that plan loads. Never a nearest-size drill — see
    // `pick_pin_tool`.
    let pin_tool =
        raw.pin_diameter
            .filter(|_| has_locating)
            .and_then(
                |diameter| match pick_pin_tool(&ctx.tools, toolset, diameter) {
                    Some(tool) => Some(tool),
                    None => {
                        warnings.push(format!(
                    "No {} drill in stock for the locating pins, and no router narrow enough \
                     to mill one. A registration hole is never made to a nearly-right size, \
                     so the step cannot run — stock a {} drill.",
                    fmt_len(ctx, diameter),
                    fmt_len(ctx, diameter),
                ));
                        None
                    }
                },
            );
    if let Some(PinTool::Router { .. }) = pin_tool.as_ref() {
        warnings.push(
            "No exact-size drill for the locating pins, so their holes are milled to size \
             with a router. Correct, but slower than drilling them."
                .into(),
        );
    }

    // Chosen here as well as in the machining plan, and by the same function, because the
    // rack has to come out identical: this view is where the operator reads which bit goes
    // in which slot.
    //
    // A step that asks to engrave and has nothing to engrave with **fails**, rather than
    // planning its other work and mentioning the gap. The program would otherwise come
    // out looking complete — drilled, routed, and not isolated — and be run by someone
    // who had no reason to doubt it. Refused for the reason a missing locating-pin drill
    // is refused: there is no degraded output worth having.
    let engraver = if raw.engraves_copper() {
        match pick_engraver(&ctx.tools, toolset, raw.engrave_copper.width) {
            Some(picked) => Some(picked),
            None => {
                return StepOutcome::Failed(vec![no_engraver_reason(
                    ctx,
                    raw.engrave_copper.width,
                )])
            }
        }
    } else {
        None
    };

    if groups.is_empty() && !has_route && engraver.is_none() && pin_tool.is_none() {
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
    let setup = build_setup(ctx, raw.fixture_id);
    // The pin tool joins the routers as mandatory: it is chosen outside the assigner, so
    // nothing else would reserve it a slot. Kept identical to the Machining plan's
    // `mandatory`, or the two views would number the rack differently.
    let mut mandatory = routers.mandatory_ids();
    if let Some(tool) = pin_tool.as_ref() {
        mandatory.push(tool.id().to_string());
    }
    if let Some((tool_id, _)) = engraver.as_ref() {
        mandatory.push(tool_id.clone());
    }
    mandatory.sort();
    mandatory.dedup();
    let rack = build_rack_spec(toolset, atc_slots, &mandatory);

    match assigner::assign(&demands, &ctx.tools, &cfg, &rack, &setup) {
        Ok(assignment) => {
            // The assigner already placed each tool on the toolset's real slot (fixed
            // tools pinned; the rest filling spare slots in order; do-not-use slots
            // skipped), so the slot numbers are used as-is.
            let number_of: std::collections::BTreeMap<&str, u8> = assignment
                .rack
                .iter()
                .map(|s| (s.tool_id.as_str(), s.slot))
                .collect();

            let rack_rows: Vec<RackRow> = assignment
                .rack
                .iter()
                .map(|s| RackRow {
                    slot: format!("T{}", s.slot),
                    tool: tool_label(ctx, &s.tool_id),
                })
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
                        routers.for_group(group),
                    );
                    RequirementRow {
                        label: group.label(ctx),
                        count: group.count,
                        tools,
                    }
                })
                .collect();

            if has_route {
                let router = routers
                    .outline
                    .as_ref()
                    .map(|id| resolve_router_tool(ctx, id, &number_of, None))
                    .unwrap_or_else(unresolved_tool);
                // Named with the kerf, because the kerf is the whole requirement: this
                // row is asking for a cutter of exactly that diameter, and a `—` beside it
                // means that cutter is not in stock rather than that some router is
                // missing.
                requirements.push(RequirementRow {
                    label: format!(
                        "Board outline ({} kerf)",
                        fmt_len(ctx, raw.route_board.kerf)
                    ),
                    count: 1,
                    tools: vec![router],
                });
            }

            for diagnostic in &assignment.diagnostics {
                warnings.push(diagnostic.message.clone());
            }

            // Distinct tools this step loads (a tool fixed in several slots appears
            // once), in slot order — for the cross-step rack schedule.
            let mut seen = std::collections::BTreeSet::new();
            let tool_ids: Vec<String> = assignment
                .rack
                .iter()
                .map(|s| s.tool_id.clone())
                .filter(|id| seen.insert(id.clone()))
                .collect();

            // Last, so a derate notice reads after the warnings about what the step cannot
            // do at all — this one is about *how* it will run, not whether it can.
            let loaded: Vec<LoadedTool> = tool_ids
                .iter()
                .filter_map(|id| ctx.tools.iter().find(|t| t.id == *id))
                .map(|tool| LoadedTool {
                    name: tool.display_name(),
                    feeds: RatedFeeds::for_tool(tool.table_feed, tool.z_feed, tool_mills(tool)),
                    speed: tool.spindle_speed,
                    // Same predicate the router selection uses, so "can it mill?" has one
                    // answer in this module rather than two that could drift.
                })
                .collect();
            warnings.extend(derate_notes(
                &machine.name,
                MachineLimits {
                    spindle: SpindleRange::new(machine.spindle_rpm_min, machine.spindle_rpm_max),
                    max_feed_xy: machine.max_feed_xy,
                    max_feed_z: machine.max_feed_z,
                },
                &loaded,
                ctx.unit_system,
            ));

            StepOutcome::Resolved(StepResolved {
                summary: machine_summary(rack_rows.len(), atc_slots),
                rack: rack_rows,
                requirements,
                warnings,
                tool_ids,
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
    slot_router: Option<&str>,
) -> Vec<ResolvedTool> {
    if group.minor.is_none() {
        return vec![resolve_drill_tool(ctx, assignment, group, number_of, None)];
    }
    // Oblong / slot: possibly a drill (ends or chain) and a router (the slot).
    let mut tools = Vec::new();
    if oblong_drills {
        tools.push(resolve_drill_tool(
            ctx,
            assignment,
            group,
            number_of,
            Some("drill"),
        ));
    }
    if oblong_routes {
        match slot_router {
            Some(router) => tools.push(resolve_router_tool(ctx, router, number_of, Some("route"))),
            // No cutter fits this slot. Show the route step unresolved rather than
            // dropping it: the requirement is real, and the step warning says why it
            // cannot be met.
            None => tools.push(ResolvedTool {
                role: Some("route"),
                ..unresolved_tool()
            }),
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
        return ResolvedTool {
            role,
            ..unresolved_tool()
        };
    };
    let slot = number_of
        .get(assigned.tool_id.as_str())
        .map(|n| format!("T{n}"))
        .unwrap_or_else(|| "—".into());
    let diameter = ctx
        .tools
        .iter()
        .find(|t| t.id == assigned.tool_id)
        .map(|t| t.diameter);
    let match_len = group.minor.unwrap_or(group.target);
    let routed = assigned.strategy == Strategy::Route;
    match diameter {
        Some(dia) => {
            let (delta_text, delta_class) = if routed {
                ("exact".to_string(), "tooling-delta-ok")
            } else {
                delta_cell(dia, match_len)
            };
            ResolvedTool {
                slot,
                role,
                diameter: fmt_len(ctx, dia),
                delta_text,
                delta_class,
                routed,
            }
        }
        None => ResolvedTool {
            slot,
            role,
            diameter: "—".into(),
            delta_text: "—".into(),
            delta_class: "",
            routed,
        },
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
    let slot = number_of
        .get(router_id)
        .map(|n| format!("T{n}"))
        .unwrap_or_else(|| "—".into());
    let diameter = ctx
        .tools
        .iter()
        .find(|t| t.id == router_id)
        .map(|t| fmt_len(ctx, t.diameter))
        .unwrap_or_else(|| "—".into());
    ResolvedTool {
        slot,
        role,
        diameter,
        delta_text: "exact".into(),
        delta_class: "tooling-delta-ok",
        routed: true,
    }
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
    let class = if pct.abs() < 2.0 {
        "tooling-delta-ok"
    } else {
        "tooling-delta-warn"
    };
    let text = if pct.abs() < 0.05 {
        format!("{pct:+.2}%")
    } else {
        format!("{pct:+.1}%")
    };
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
    pub(crate) fn from_hole(
        hole: &pcb::BoardHole,
        has_pth: bool,
        has_npth: bool,
    ) -> Option<HoleGroup> {
        let kind = match hole.kind {
            pcb::HoleKind::PadPth | pcb::HoleKind::Via if has_pth => DemandKind::Pth,
            pcb::HoleKind::PadNpth if has_npth => DemandKind::Npth,
            _ => return None,
        };
        let (major, _) = hole.drill_axes()?;
        // `BoardHole::slot` is the single oblong classification for the whole app, so
        // this group and the machining plan's chain geometry can never disagree about
        // which holes are slots.
        let minor = hole.slot().map(|slot| slot.width);
        Some(HoleGroup {
            kind,
            target: major,
            minor,
            count: 1,
        })
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
pub(crate) fn collect_hole_groups(
    holes: &[pcb::BoardHole],
    has_pth: bool,
    has_npth: bool,
) -> Vec<HoleGroup> {
    let mut groups: Vec<HoleGroup> = Vec::new();
    for hole in holes {
        let Some(group) = HoleGroup::from_hole(hole, has_pth, has_npth) else {
            continue;
        };
        let target_um = micron(group.target);
        let minor_um = group.minor.map(micron).unwrap_or(-1);
        if let Some(existing) = groups.iter_mut().find(|g| {
            g.kind == group.kind
                && micron(g.target) == target_um
                && g.minor.map(micron).unwrap_or(-1) == minor_um
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
fn pick_outline_router(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    kerf: Length,
) -> Option<String> {
    // **The kerf is the cutter.** A single-pass route removes exactly one cutter width of
    // material, so a 2 mm kerf is a 2 mm router and nothing else will do — matched to 1 µm,
    // the assigner's own precision.
    //
    // Refused rather than substituted, the same rule locating pins follow: a kerf is a
    // dimension of the finished job, and a board cut with a 1.6 mm channel where 2 mm was
    // specified is a different board. The caller turns a `None` into a step failure naming
    // the size, which is a fix the operator can act on; quietly cutting narrower is not.
    //
    // This replaced "the smallest router in stock, or any router pinned in the toolset".
    // Both were wrong for an edge cut: the smallest cutter is the right default for
    // reaching tight *internal* corners and the slowest possible way to cut an outline,
    // and a pinned 0.8 would win over everything — the operator's request losing to a
    // convenience.
    let wanted_um = micron(kerf);
    let exact = |tool: &&Tool| micron(tool.diameter) == wanted_um;

    // Being already in the toolset is worth a tie-break between equals — it costs no rack
    // slot — but it is only ever a tie-break now, since only one diameter qualifies.
    fixed_routers(tools, toolset)
        .find(|t| exact(t))
        .or_else(|| stock_routers(tools).find(exact))
        .map(|t| t.id.clone())
}

/// Whether a tool can mill (as opposed to drill).
///
fn is_router_tool(tool: &Tool) -> bool {
    matches!(
        ToolKind::from_kind_label(&tool.kind),
        ToolKind::Routerbit | ToolKind::Endmill
    )
}

/// Whether `tool` cuts with its flutes rather than its point.
///
/// **Not the same question as [`is_router_tool`]**, though it was the same predicate until
/// V-bits arrived, and the difference is a broken tool.
///
/// `is_router_tool` answers "may this be *chosen* to route an outline or a slot", where a
/// V-bit is the wrong answer — it cuts a vee, not a channel of its own diameter.
///
/// This one answers "does it enter the work sideways", which decides how
/// [`RatedFeeds::for_tool`](crate::gcode::feeds::RatedFeeds::for_tool) fills in a missing
/// plunge rating: a side-cutting tool gets the derated
/// [`PLUNGE_FEED_FRACTION`](crate::gcode::routing), a drill gets the full lateral feed
/// because plunging is the only thing it does. Answer that wrongly for a V-bit and an
/// unrated engraver drives its tip straight down at cutting speed — which is how you
/// discover the two questions were never the same one.
pub(crate) fn tool_mills(tool: &Tool) -> bool {
    matches!(
        ToolKind::from_kind_label(&tool.kind),
        ToolKind::Routerbit | ToolKind::Endmill | ToolKind::Vbit | ToolKind::Engraver
    )
}

/// Routers pinned in the toolset's slots, in slot order. These are already in the rack,
/// so choosing one costs no slot.
fn fixed_routers<'a>(
    tools: &'a [Tool],
    toolset: &'a crate::data::model::ToolsetProfile,
) -> impl Iterator<Item = &'a Tool> + 'a {
    toolset
        .slots
        .values()
        .filter_map(|slot| slot.tool_id.as_ref())
        .filter_map(|id| tools.iter().find(|t| &t.id == id && is_router_tool(t)))
}

/// Every in-stock router.
fn stock_routers(tools: &[Tool]) -> impl Iterator<Item = &Tool> {
    tools
        .iter()
        .filter(|t| is_router_tool(t) && t.status == crate::data::model::ToolStatus::InStock)
}

/// Whether `tool` is a bit that cuts a vee — the only kind isolation engraving uses.
///
/// The opposite side of the split [`tool_mills`] describes: a router has one width and a
/// V-bit has a width per depth, and that is exactly why engraving wants the V-bit. Asked
/// separately from `is_router_tool` so neither list can quietly acquire the other's tools.
fn is_engraver_tool(tool: &Tool) -> bool {
    matches!(
        ToolKind::from_kind_label(&tool.kind),
        ToolKind::Vbit | ToolKind::Engraver
    )
}

/// The V-bit that cuts an isolation channel `width` across, and how deep to sink it.
///
/// Two constraints, both hard. The tip must be **no wider than the channel** — a bit
/// already broader than the cut it is asked for cannot make it at any depth — and the
/// depth that arithmetic asks for must be one the bit is **rated to hold**
/// (`z_min_depth`); a cut shallower than the tool's rating is a cut whose width nobody can
/// promise, and width is the whole product here.
///
/// Among the bits that qualify, **the largest tip wins, then the shallowest cone**. Both
/// halves are the same argument, which is that depth error is width error here: a wide tip
/// spends less of the channel on cone, and a shallow cone opens out more slowly per unit
/// of depth. On a 90° bit a tenth of a millimetre of board bow moves the trace width by
/// two tenths; on a 15° one it moves it by two hundredths. Width is the product, so that
/// sensitivity is what is being chosen against — not stiffness, which would argue the
/// other way on the angle.
///
/// Toolset-fixed bits are tried first because they cost no rack slot, the same tie-break
/// [`pick_slot_router`] makes.
pub(crate) fn pick_engraver(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    width: Length,
) -> Option<(String, Length)> {
    let suits = |tool: &&Tool| -> Option<(String, Length)> {
        // The bit's diameter *is* its tip: the narrowest channel it can cut, widening
        // from there with depth. See the V-bit note in the generic catalogue.
        let depth_mm = crate::gcode::assigner::engrave_depth_mm(
            tool.diameter.as_mm(),
            tool.point_angle.as_degrees(),
            width.as_mm(),
        )?;
        if depth_mm <= 0.0 {
            return None;
        }
        if tool.z_min_depth.is_some_and(|min| depth_mm < min.as_mm()) {
            return None;
        }
        Some((tool.id.clone(), Length::from_mm(depth_mm)))
    };

    // Widest tip first and shallowest cone next, so the first that suits is the best that
    // suits. The final tie breaks on the tool id, which keeps the choice a total function
    // of the stock list rather than of the order it happens to be stored in.
    let mut fixed: Vec<&Tool> = toolset
        .slots
        .values()
        .filter_map(|slot| slot.tool_id.as_ref())
        .filter_map(|id| tools.iter().find(|t| &t.id == id && is_engraver_tool(t)))
        .collect();
    let mut stock: Vec<&Tool> = tools
        .iter()
        .filter(|t| is_engraver_tool(t) && t.status == crate::data::model::ToolStatus::InStock)
        .collect();
    let widest_tip_first = |a: &&Tool, b: &&Tool| {
        let tip = |t: &Tool| std::cmp::Reverse(micron(t.diameter));
        let angle = |t: &Tool| (t.point_angle.as_degrees() * 1e3) as i64;
        tip(a)
            .cmp(&tip(b))
            .then_with(|| angle(a).cmp(&angle(b)))
            .then_with(|| a.id.cmp(&b.id))
    };
    fixed.sort_by(widest_tip_first);
    stock.sort_by(widest_tip_first);

    fixed.iter().find_map(suits).or_else(|| stock.iter().find_map(suits))
}

/// The router that mills a slot `width` across: the **largest** cutter that still fits.
///
/// `diameter <= width` is a hard constraint, not a preference — a cutter wider than the
/// slot cannot enter it at all, so the board-outline router is no guide here (a 1.2 mm
/// outline cutter cannot touch a 0.4 mm slot). Among those that do fit, the largest is
/// the stiffest and needs the fewest passes; one exactly the slot width mills it in a
/// single pass down the centre line. A toolset-fixed router that fits beats a larger
/// in-stock one, since it is already in the rack.
fn pick_slot_router(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    width: Length,
) -> Option<String> {
    let limit_um = micron(width);
    if let Some(tool) = fixed_routers(tools, toolset).find(|t| micron(t.diameter) <= limit_um) {
        return Some(tool.id.clone());
    }
    stock_routers(tools)
        .filter(|t| micron(t.diameter) <= limit_um)
        .max_by_key(|t| micron(t.diameter))
        .map(|t| t.id.clone())
}

/// How a locating-pin hole gets made.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PinTool {
    /// A drill of **exactly** the pin diameter.
    Drill { id: String, diameter: Length },
    /// No exact drill exists, so the hole is milled to size by spiralling a router.
    Router { id: String, diameter: Length },
}

impl PinTool {
    pub(crate) fn id(&self) -> &str {
        match self {
            PinTool::Drill { id, .. } | PinTool::Router { id, .. } => id,
        }
    }
}

/// The tool that makes a locating-pin hole of `diameter`.
///
/// **A registration hole is never made to a nearly-right size.** Everywhere else in k2g a
/// drill within the step's oversize/undersize allowance is a perfectly good substitute,
/// because a component lead does not care about 40 µm. A pin does: the whole point of the
/// hole is that the board returns to exactly where it was, and a hole 0.1 mm over lets it
/// return to anywhere within 0.1 mm. So the allowance machinery is deliberately not
/// consulted here.
///
/// The order is therefore *exact drill*, then *router*, then nothing:
///
/// 1. a drill of exactly the pin diameter (to 1 µm, the assigner's own precision),
///    preferring one already fixed in the toolset since it costs no slot;
/// 2. else the largest router that fits, which spirals the hole to exact size — a
///    different way of being exact, not a substitution;
/// 3. else `None`, and the caller refuses the step. Better no program than a board with
///    registration holes that do not register.
pub(crate) fn pick_pin_tool(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    diameter: Length,
) -> Option<PinTool> {
    let wanted_um = micron(diameter);
    // `Drillbit` specifically, not merely "not a router". [`ToolKind::from_kind_label`]
    // falls through to `Endmill` for anything it does not recognise, so "not a router"
    // would silently accept a V-bit, an engraver, or a tool whose kind string is a typo —
    // none of which produce the straight-walled hole a pin has to seat in.
    let exact = |tool: &&Tool| {
        matches!(ToolKind::from_kind_label(&tool.kind), ToolKind::Drillbit)
            && micron(tool.diameter) == wanted_um
    };

    // Fixed in the toolset first, for the same reason the outline router is: it is already
    // in the rack, so choosing it costs no slot.
    let fixed_exact = toolset
        .slots
        .values()
        .filter_map(|slot| slot.tool_id.as_ref())
        .filter_map(|id| tools.iter().find(|t| &t.id == id))
        .find(|t| exact(t));
    let drill = fixed_exact.or_else(|| {
        tools
            .iter()
            .filter(|t| t.status == crate::data::model::ToolStatus::InStock)
            .find(exact)
    });
    if let Some(tool) = drill {
        return Some(PinTool::Drill {
            id: tool.id.clone(),
            diameter: tool.diameter,
        });
    }

    // Milled to size instead. Largest that fits, as for a slot: stiffest, fewest passes.
    pick_slot_router(tools, toolset, diameter).map(|id| PinTool::Router {
        diameter: tools
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.diameter)
            .unwrap_or(diameter),
        id,
    })
}

/// Which router each of a step's routed features needs.
///
/// Resolved once and shared by the Tooling tab and the Machining plan: both build the
/// rack from [`RouterPlan::mandatory_ids`], so the two views cannot disagree about which
/// tools are loaded or which slot each lands in.
#[derive(Default)]
pub(crate) struct RouterPlan {
    /// The board-outline router, when the step routes the outline.
    pub(crate) outline: Option<String>,
    /// Slot width (µm) → the router that mills it. A width is absent when nothing fits.
    by_slot_width_um: std::collections::BTreeMap<i64, String>,
    /// Slot widths no available router is small enough to mill.
    pub(crate) unroutable_widths: Vec<Length>,
    /// Contour index → the router that cuts that interior opening, and what it leaves.
    ///
    /// Per cutout rather than one for all of them, because a cutout's cutter is chosen by
    /// **fit**: a board with a 6 mm window and a 1.5 mm slot wants two different cutters,
    /// and forcing one on both means either failing the slot or crawling round the window.
    pub(crate) cutouts: std::collections::BTreeMap<usize, (String, pcb::CutoutFit)>,
    /// Cutout indices no available router can cut.
    pub(crate) uncuttable_cutouts: Vec<usize>,
    /// Contour index → the relief drill for each of its convex corners, in the order
    /// [`pcb::convex_corners`] reports them. `None` where no drill suits.
    ///
    /// Resolved here rather than at plan time because these drills have to be **reserved**:
    /// a step that only cuts openings drills nothing else, so its rack would otherwise hold
    /// routers alone and every corner would be skipped for want of a tool that was in stock
    /// all along. Mouse bites can afford to take whatever is already loaded; corner relief
    /// is the only reason such a step needs a drill at all.
    pub(crate) corner_drills: std::collections::BTreeMap<usize, Vec<Option<String>>>,
}

impl RouterPlan {
    /// The router milling this group's slot — `None` for a round hole, and also for a
    /// slot too narrow for every available cutter (see [`Self::unroutable_widths`]).
    pub(crate) fn for_group(&self, group: &HoleGroup) -> Option<&str> {
        let width = group.minor?;
        self.by_slot_width_um
            .get(&micron(width))
            .map(String::as_str)
    }

    /// Every router the rack must hold, deduplicated and in a stable order.
    pub(crate) fn mandatory_ids(&self) -> Vec<String> {
        let mut ids: std::collections::BTreeSet<String> =
            self.by_slot_width_um.values().cloned().collect();
        ids.extend(self.outline.clone());
        ids.extend(self.cutouts.values().map(|(id, _)| id.clone()));
        ids.extend(self.corner_drills.values().flatten().flatten().cloned());
        ids.into_iter().collect()
    }
}

/// Why a step that asks to engrave cannot.
///
/// One wording, used by both the Tooling tab and the machining plan, so the operator does
/// not meet two accounts of one fault. It names the way out, which a blocking error owes
/// the reader: this one is fixed by stocking a finer bit or by asking for a wider channel,
/// and nothing about the board or the profile will help.
pub(crate) fn no_engraver_reason(ctx: &AppState, width: Length) -> String {
    format!(
        "No V-bit in stock can cut a {} isolation channel: every tip is either wider than \
         that, or would have to be sunk shallower than the bit is rated for. Stock a finer \
         V-bit, or widen the channel on this step.",
        fmt_len(ctx, width),
    )
}

/// The band of drill diameters that relieves a corner of interior angle `θ` cut by a
/// router of diameter `d`, as `(minimum, ideal)` in millimetres.
///
/// The ideal sits the drill tangent to both edges with its centre on the arc the router
/// leaves. The minimum is where the drilled hole stops overlapping that arc at all: below
/// it the hole is an island of its own with an uncut web between it and the pocket, which
/// comes out as a loose chip rather than a sharper corner.
pub(crate) fn corner_relief_band(router_diameter_mm: f64, interior_rad: f64) -> Option<(f64, f64)> {
    let s = (interior_rad / 2.0).sin();
    if s <= f64::EPSILON {
        return None;
    }
    let ideal = router_diameter_mm * (1.0 - s);
    (ideal > 0.0).then_some((ideal / (1.0 + s), ideal))
}

/// The largest drill in the band, preferring one already racked.
///
/// Largest because within the band a smaller drill's extra reach is second-order while
/// merging cleanly with the routed pocket is not, and a wider drill is stiffer.
fn pick_corner_drill(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    min_mm: f64,
    ideal_mm: f64,
) -> Option<String> {
    let suits = |t: &&Tool| {
        matches!(
            crate::data::model::tool_core::ToolKind::from_kind_label(&t.kind),
            crate::data::model::tool_core::ToolKind::Drillbit
        ) && t.diameter.as_mm() >= min_mm
            && t.diameter.as_mm() <= ideal_mm
    };
    let racked: Vec<&Tool> = toolset
        .slots
        .values()
        .filter_map(|slot| slot.tool_id.as_ref())
        .filter_map(|id| tools.iter().find(|t| &t.id == id))
        .collect();
    racked
        .iter()
        .copied()
        .filter(suits)
        .max_by_key(|t| micron(t.diameter))
        .map(|t| t.id.clone())
        .or_else(|| {
            tools
                .iter()
                .filter(|t| t.status == crate::data::model::ToolStatus::InStock)
                .filter(suits)
                .max_by_key(|t| micron(t.diameter))
                .map(|t| t.id.clone())
        })
}

/// The router that cuts one interior opening: the largest that can reach all of it,
/// preferring one already loaded.
///
/// Chosen by **fit**, which is the opposite of how the edge kerf is chosen. A kerf is a
/// dimension of the finished job, so it is matched exactly and a missing cutter fails the
/// step. A cutout has no such dimension — it is a hole in the middle of a board, and the
/// only thing asked of the cutter is that it reaches everywhere the drawing says is gone.
///
/// Already-racked first and only then largest, because the wall pass is a single lap
/// whatever the diameter: a wider cutter buys stiffness, never fewer passes, and that is
/// not worth a tool change to get. The outline's cutter is tried before either, since a
/// step that also routes the edge already has it loaded.
///
/// A candidate is accepted when [`pcb::fit_cutout`] reaches everywhere and strands no
/// more than one cutter-radius square of material beyond the fillets its own roundness
/// forces it to leave at each corner. That budget is generous by design: real unreachable
/// material — a 1 mm arm the cutter cannot enter — is many times it, while the numerical
/// error of two chained offsets is far below.
fn pick_cutout_router(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    contour: &pcb::Contour,
    outline_router: Option<&str>,
) -> Option<(String, pcb::CutoutFit)> {
    let mut candidates: Vec<&Tool> = Vec::new();
    if let Some(id) = outline_router {
        candidates.extend(tools.iter().find(|t| t.id == id));
    }
    let mut racked: Vec<&Tool> = fixed_routers(tools, toolset).collect();
    racked.sort_by_key(|t| std::cmp::Reverse(micron(t.diameter)));
    candidates.extend(racked);
    let mut stock: Vec<&Tool> = stock_routers(tools).collect();
    stock.sort_by_key(|t| std::cmp::Reverse(micron(t.diameter)));
    candidates.extend(stock);

    for tool in candidates {
        let radius_nm = (tool.diameter.as_nm() / 2.0).round() as i64;
        if radius_nm <= 0 {
            continue;
        }
        let Some(fit) = pcb::fit_cutout(contour, radius_nm) else {
            continue;
        };
        let budget = (radius_nm as f64) * (radius_nm as f64);
        if fit.excess_nm2 <= budget {
            return Some((tool.id.clone(), fit));
        }
    }
    None
}

/// What the interior openings need from the router planner.
///
/// A pair rather than two parameters because they are one decision: the corner drills are
/// sized from the cutter each opening gets, so asking for relief without the contours is
/// meaningless.
pub(crate) struct CutoutRouting<'a> {
    pub(crate) contours: &'a [(usize, &'a pcb::Contour)],
    /// Reserve a relief drill for each corner the cutter will round off.
    pub(crate) relieve_corners: bool,
}

/// Resolves a step's routing tools: the outline router (when it routes the outline) and
/// one router per distinct slot width (when its oblong strategy mills slots).
pub(crate) fn plan_routers(
    tools: &[Tool],
    toolset: &crate::data::model::ToolsetProfile,
    groups: &[HoleGroup],
    has_route: bool,
    kerf: Length,
    oblong_routes: bool,
    cutouts: CutoutRouting<'_>,
) -> RouterPlan {
    let mut plan = RouterPlan::default();

    if has_route {
        plan.outline = pick_outline_router(tools, toolset, kerf);
    }

    // Resolved here rather than in the planner so the Tooling tab and the machining plan
    // reserve the same slots — the rack is built from `mandatory_ids`, and a cutter the
    // two disagree about is a cutter with no slot number.
    let CutoutRouting { contours: cutouts, relieve_corners } = cutouts;
    for (index, contour) in cutouts {
        match pick_cutout_router(tools, toolset, contour, plan.outline.as_deref()) {
            Some((tool_id, fit)) => {
                if relieve_corners {
                    let diameter_mm = tools
                        .iter()
                        .find(|t| t.id == tool_id)
                        .map(|t| t.diameter.as_mm())
                        .unwrap_or(0.0);
                    let drills = pcb::convex_corners(contour, pcb::MIN_CORNER_TURN_RAD)
                        .iter()
                        .map(|corner| {
                            let (min, ideal) =
                                corner_relief_band(diameter_mm, corner.interior_rad)?;
                            pick_corner_drill(tools, toolset, min, ideal)
                        })
                        .collect();
                    plan.corner_drills.insert(*index, drills);
                }
                plan.cutouts.insert(*index, (tool_id, fit));
            }
            None => plan.uncuttable_cutouts.push(*index),
        }
    }

    if oblong_routes {
        // One lookup per distinct width — a board typically has a handful of slot sizes
        // and they very often share a cutter.
        let mut seen = std::collections::BTreeSet::new();
        for group in groups {
            let Some(width) = group.minor else { continue };
            if !seen.insert(micron(width)) {
                continue;
            }
            match pick_slot_router(tools, toolset, width) {
                Some(id) => {
                    plan.by_slot_width_um.insert(micron(width), id);
                }
                None => plan.unroutable_widths.push(width),
            }
        }
    }

    plan
}

/// The interior openings this step is responsible for, paired with their contour index.
///
/// Empty unless the step runs `route_cutouts`: without that operation the openings are
/// the edge router's business and are cut to its kerf, so there is no per-cutout cutter
/// to choose and nothing to reserve.
pub(crate) fn cutout_contours<'a>(
    stitched: Option<&'a pcb::StitchResult>,
    raw: &StepRaw,
) -> Vec<(usize, &'a pcb::Contour)> {
    if !raw.routes_cutouts() {
        return Vec::new();
    }
    let Some(stitched) = stitched else {
        return Vec::new();
    };
    if !stitched.errors.is_empty() {
        return Vec::new();
    }
    stitched
        .contours
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_hole)
        .collect()
}

/// Why an opening could not be cut, naming the smallest router in stock so the fix reads
/// straight off the message.
fn uncuttable_cutout_warning(ctx: &AppState, index: usize) -> String {
    match stock_routers(&ctx.tools).min_by_key(|t| micron(t.diameter)) {
        Some(smallest) => format!(
            "Cutout #{index} cannot be cut by any router in stock — the smallest ({}) still \
             cannot reach all of it. Stock a smaller router, or widen the opening.",
            fmt_len(ctx, smallest.diameter),
        ),
        None => format!("Cutout #{index} cannot be cut: there is no router in stock."),
    }
}

/// Why a slot could not be milled, naming the width and the closest cutter so the fix —
/// stock a smaller router, or switch the step to a drill-only oblong strategy — is
/// readable straight off the message.
fn unroutable_slot_warning(ctx: &AppState, width: Length) -> String {
    match stock_routers(&ctx.tools).min_by_key(|t| micron(t.diameter)) {
        Some(smallest) => format!(
            "Slot {} is narrower than the smallest router in stock ({}), so it cannot be milled. \
             Stock a router no larger than {}, or set this step's oblong strategy to drill only.",
            fmt_len(ctx, width),
            fmt_len(ctx, smallest.diameter),
            fmt_len(ctx, width),
        ),
        None => format!(
            "Slot {} cannot be milled — no router in stock.",
            fmt_len(ctx, width)
        ),
    }
}

/// One tool as the derate notice needs it: what to call it, and its rated pair.
///
/// Owned rather than a `&Tool` because the caller has already reduced the step's rack to
/// distinct ids, and carrying the whole tool here would invite this to grow into a second
/// tool model.
pub(crate) struct LoadedTool {
    pub name: String,
    /// The tool's two rated feeds. The derate notice reports on the **lateral** one,
    /// which is the number a datasheet leads with and the one an operator compares
    /// against; the plunge scales by the same ratio, so one line says both.
    pub feeds: RatedFeeds,
    pub speed: Option<RotationalSpeed>,
}

/// Tells the operator that the step will not run at its tools' rated values.
///
/// A tool's rated feed is defined *at its rated spindle speed*, so when the machine cannot
/// deliver that pair [`feeds::resolve`] moves the spindle and scales the feed with it,
/// holding chip load constant. That is correct and deliberate — but it is also invisible:
/// the program simply carries a smaller `F` than the datasheet, with nothing to
/// distinguish a deliberate derate from a bug.
///
/// **Aggregated on purpose.** A catalog rating drills at 48–100 kRPM against a slower
/// spindle clamps most of a step's tools, so one warning per tool would be nine lines of
/// alarm for the entirely normal case, burying the step's real warnings (no router in
/// stock, unroutable widths, assigner diagnostics). One line per cause, naming the worst
/// offender, says the same thing.
///
/// The causes are separate lines because they mean different things and have different
/// fixes: capping is routine, a tool rated *below* the spindle floor is run faster than it
/// is rated for, a feed ceiling means the job is slower than it needs to be (raise the
/// axis limit if the machine really is faster), and a conflict means the chip load is not
/// being met at all.
///
/// Tools whose rated pair is incomplete are skipped rather than reported: generation
/// already fails those with a message of its own ([`crate::gcode::program::BodyError`]),
/// and repeating it here would be noise.
pub(crate) fn derate_notes(
    machine: &str,
    limits: MachineLimits,
    tools: &[LoadedTool],
    unit: UserUnitSystem,
) -> Vec<String> {
    // (tool name, scale, rated feed) per cause, where scale is running rpm ÷ rated rpm.
    let mut capped: Vec<(&str, f64, FeedRate)> = Vec::new();
    let mut feed_bound: Vec<(&str, f64, FeedRate)> = Vec::new();
    // Ranked by rated *speed* rather than by scale: nothing is scaled for these, so every
    // entry's scale is 1.0 and picking the "deepest" by it would name an arbitrary tool.
    // The slowest tool is the one furthest outside what the machine can do.
    let mut below_floor: Vec<(&str, RotationalSpeed, FeedRate)> = Vec::new();

    for tool in tools {
        let Ok(resolved) = feeds::resolve(tool.feeds, tool.speed, limits) else {
            continue;
        };
        // Both are `Some` — `resolve` would have errored otherwise.
        let (Some(rated_feed), Some(rated_speed)) = (tool.feeds.table, tool.speed) else {
            continue;
        };
        let scale = resolved.rpm.as_rpm() / rated_speed.as_rpm();
        let entry = (tool.name.as_str(), scale, rated_feed);
        match resolved.limit {
            Limited::No => {}
            Limited::Spindle => capped.push(entry),
            Limited::Feed => feed_bound.push(entry),
            Limited::SpindleFloor => {
                below_floor.push((tool.name.as_str(), rated_speed, rated_feed))
            }
        }
    }

    let total = tools.len();
    let mut notes = Vec::new();
    // The worst case is the one to sanity-check: the deepest cut in speed, or the largest
    // increase where the spindle floor pushed tools up.
    let deepest = |group: &[(&str, f64, FeedRate)]| {
        group
            .iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|w| (w.0.to_string(), w.1, w.2))
    };

    if let Some((name, scale, rated)) = deepest(&capped) {
        notes.push(format!(
            "{} of {total} tool(s) exceed {machine}'s {} ceiling — the spindle is capped and \
             feeds are derated in proportion to hold chip load. Deepest: {name} at {} of its \
             rated {}.",
            capped.len(),
            unit_format::format_rotational_speed_display(limits.spindle.max),
            percent(scale),
            rated.unit_display(unit).user,
        ));
    }
    if let Some((name, _, rated)) = deepest(&feed_bound) {
        notes.push(format!(
            "{} of {total} tool(s) want a feed faster than {machine} can move ({} in XY, {} on Z) \
             — the feed is capped and the spindle keeps its speed, so they cut lighter than rated. \
             Worst: {name}, rated {}. Raise the machine's feed limits if it really is faster.",
            feed_bound.len(),
            limits.max_feed_xy.unit_display(unit).user,
            limits.max_feed_z.unit_display(unit).user,
            rated.unit_display(unit).user,
        ));
    }
    // Nothing was altered for these — the program carries the tool's own rated pair. What
    // the machine does with a speed below its floor is the machine's business, and that is
    // exactly why it is worth saying out loud.
    let slowest = below_floor
        .iter()
        .min_by(|a, b| a.1.as_rpm().total_cmp(&b.1.as_rpm()));
    if let Some((name, rated_speed, rated_feed)) = slowest {
        notes.push(format!(
            "{} of {total} tool(s) are rated below {machine}'s {} minimum — the program still \
             commands their rated speed, but the machine will not turn that slowly, so the cut \
             will be heavier per revolution than rated. Slowest: {name} at {}, rated {}. Use a \
             tool the spindle can run, or correct the machine's minimum.",
            below_floor.len(),
            unit_format::format_rotational_speed_display(limits.spindle.min),
            unit_format::format_rotational_speed_display(*rated_speed),
            rated_feed.unit_display(unit).user,
        ));
    }
    notes
}

/// A feed/speed ratio as a percentage, without a trailing `.0` on the whole numbers that
/// round ratings usually produce.
fn percent(scale: f64) -> String {
    let value = scale * 100.0;
    if (value - value.round()).abs() < 0.05 {
        format!("{}%", value.round())
    } else {
        format!("{value:.1}%")
    }
}

/// Maps a toolset + ATC count to the assigner's rack spec. Capacity = usable
/// (non-disabled) slots, capped by the ATC size when the machine has one.
pub(crate) fn build_rack_spec(
    toolset: &crate::data::model::ToolsetProfile,
    atc_slots: usize,
    mandatory: &[String],
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
    let capacity = if atc_slots > 0 {
        placeable.min(atc_slots)
    } else {
        placeable
    };

    RackSpec {
        capacity,
        fixed,
        spare_slots,
        mandatory: mandatory.to_vec(),
        policy: map_policy(toolset.generation_policy),
    }
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

/// A specific, numeric reason for a depth-infeasible drill — names *which* half of
/// Z-feasibility failed so the operator knows what to change (a longer bit vs. more
/// below-board space). Lengths are in mm (the CAM working unit).
fn depth_reason(d: DepthDetail) -> String {
    if !d.bed_ok {
        // Bed safety: the point + breakthrough would drive past the usable space.
        format!(
            "would reach the machine bed — a \u{2300}{:.2}mm {:.0}\u{00b0} drill breaks through {:.2}mm below the board but only {:.2}mm of below-board space is available (raise the backboard thickness, lower the bed clearance, or route it)",
            d.diameter_mm, d.point_angle_deg, d.breakthrough_mm, d.bed_space_mm
        )
    } else {
        // Reach: the flute cannot plunge deep enough to finish the hole.
        let flute = d
            .flute_mm
            .map(|f| format!("{f:.2}mm"))
            .unwrap_or_else(|| "unset".to_string());
        format!(
            "the drill is too short — a \u{2300}{:.2}mm bit needs {:.2}mm of reach to break through but its flute is only {flute} (use a longer bit or route it)",
            d.diameter_mm, d.needed_plunge_mm
        )
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
                    FaultReason::NoSizeMatch => "no in-stock drill matches within the allowance and routing is unavailable".to_string(),
                    FaultReason::DepthInfeasible => fault
                        .depth
                        .map(depth_reason)
                        .unwrap_or_else(|| "the matching drill is too short to reach through, or would hit the bed".to_string()),
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

/// A non-negative integer node, as a count. Negative values are rejected rather than
/// wrapped — a count is what the schema's `minimum: 0` fields all are.
fn node_count(root: &Node, ptr: &str) -> Option<usize> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Int(value) => usize::try_from(*value).ok(),
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

/// Reads the locating-pin diameter, which is stored as one of a fixed set of **strings**
/// (`"3.2mm"`) rather than as a unit-bearing node.
///
/// It is an `enum` in the schema and not a `units.yaml` `$ref`, so that the editor offers
/// the pin sizes that exist rather than a box to type any diameter into — see the comment
/// on `pin_diameter` in `schemas/machining.yaml`. The cost lands here: the value arrives
/// as text and has to be parsed. [`node_length`] is still tried, so a profile whose
/// diameter was somehow stored typed is read rather than silently dropped.
fn node_pin_diameter(root: &Node, ptr: &str) -> Option<Length> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Str(s) => Length::from_string(s, Some(units::LengthUnit::Mm)).ok(),
        _ => node_length(root, ptr),
    }
}

/// Reads a `percent` value (stored untyped, usually `"8%"`) as a fraction (`0.08`).
fn node_percent_fraction(root: &Node, ptr: &str) -> Option<f64> {
    match &root.get_pointer(ptr)?.value {
        NodeValue::Str(s) => s
            .trim()
            .trim_end_matches('%')
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| v / 100.0),
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
            position: BoardPoint {
                x: Length::from_mm(0.0),
                y: Length::from_mm(0.0),
            },
            drill_x: Some(Length::from_mm(dx_mm)),
            drill_y: Some(Length::from_mm(dy_mm)),
            plated: None,
            orientation_deg: None,
        }
    }

    // --- derate notices ----------------------------------------------------

    /// A loaded **drill**: one rated feed, which is therefore also its plunge feed.
    fn loaded(name: &str, feed_mm_min: f64, rpm: f64) -> LoadedTool {
        LoadedTool {
            name: name.to_string(),
            feeds: RatedFeeds::for_tool(Some(FeedRate::from_mm_per_min(feed_mm_min)), None, false),
            speed: Some(RotationalSpeed::from_rpm(rpm)),
        }
    }

    /// A loaded **router**: the same one rated feed, but its plunge is a third of it.
    fn loaded_router(name: &str, feed_mm_min: f64, rpm: f64) -> LoadedTool {
        LoadedTool {
            feeds: RatedFeeds::for_tool(Some(FeedRate::from_mm_per_min(feed_mm_min)), None, true),
            ..loaded(name, feed_mm_min, rpm)
        }
    }

    /// Spindle limits only: the axes are set far beyond anything a test tool asks for, so
    /// a test about the spindle range is not silently also a test about the feed ceiling.
    fn spindle(min: f64, max: f64) -> MachineLimits {
        machine(min, max, 1e9, 1e9)
    }

    fn machine(rpm_min: f64, rpm_max: f64, xy: f64, z: f64) -> MachineLimits {
        MachineLimits {
            spindle: SpindleRange::new(
                RotationalSpeed::from_rpm(rpm_min),
                RotationalSpeed::from_rpm(rpm_max),
            ),
            max_feed_xy: FeedRate::from_mm_per_min(xy),
            max_feed_z: FeedRate::from_mm_per_min(z),
        }
    }

    /// The case this feature exists for: the catalog rates drills far above the spindle,
    /// so everything is capped. One line, not one per tool.
    #[test]
    fn every_capped_tool_collapses_into_a_single_note() {
        let tools = vec![
            loaded("1.0mm drill", 14_400.0, 48_000.0),  // → 50%
            loaded("0.8mm drill", 17_100.0, 57_000.0),  // → 42.1%
            loaded("0.3mm drill", 20_000.0, 100_000.0), // → 24%, the deepest
        ];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert_eq!(
            notes.len(),
            1,
            "one aggregated line, not one per tool: {notes:#?}"
        );
        let note = &notes[0];
        assert!(note.contains("3 of 3 tool(s)"), "counts them: {note}");
        assert!(note.contains("24000 rpm"), "names the ceiling: {note}");
        assert!(
            note.contains("0.3mm drill"),
            "names the worst offender: {note}"
        );
        assert!(note.contains("24%"), "reports the deepest derate: {note}");
        // Unspaced, which is how `UserUnitDisplay` renders a feed everywhere else.
        assert!(
            note.contains("20000mm/min"),
            "against the rated feed: {note}"
        );
    }

    /// Over the ceiling and under the floor are opposite problems with opposite fixes, so
    /// they never share a line. Only the first is something the application does anything
    /// about — a tool under the floor is reported and left exactly as rated.
    #[test]
    fn being_over_the_ceiling_and_under_the_floor_are_reported_separately() {
        let tools = vec![
            loaded("1.0mm drill", 14_400.0, 48_000.0), // above the ceiling → scaled
            loaded("slow cutter", 200.0, 1_000.0),     // below the floor   → reported only
        ];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert_eq!(notes.len(), 2, "one per direction: {notes:#?}");
        assert!(notes[0].contains("capped"), "capping first: {}", notes[0]);
        assert!(notes[1].contains("below"), "then the floor: {}", notes[1]);
        assert!(notes[1].contains("slow cutter"), "{}", notes[1]);
        assert!(
            notes[1].contains("still commands their rated speed"),
            "must say nothing was changed: {}",
            notes[1]
        );
    }

    /// The guard against alarm fatigue: a machine that can reach the ratings says nothing.
    #[test]
    fn nothing_clamped_produces_no_note_at_all() {
        let tools = vec![loaded("1.0mm drill", 600.0, 12_000.0)];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert!(
            notes.is_empty(),
            "rated speed is reachable — nothing to say: {notes:#?}"
        );
    }

    /// A missing rating is generation's error to report, with its own message. Counting it
    /// here would double-report it, and the total must stay honest either way.
    #[test]
    fn a_tool_missing_its_rating_is_skipped_not_counted() {
        let tools = vec![
            loaded("1.0mm drill", 14_400.0, 48_000.0),
            LoadedTool {
                name: "no feed".into(),
                feeds: RatedFeeds::new(None, None),
                speed: Some(RotationalSpeed::from_rpm(60_000.0)),
            },
            LoadedTool {
                name: "no speed".into(),
                feeds: RatedFeeds::new(Some(FeedRate::from_mm_per_min(400.0)), None),
                speed: None,
            },
        ];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("1 of 3 tool(s)"),
            "only the rated one is counted: {}",
            notes[0]
        );
        assert!(
            !notes[0].contains("no feed") && !notes[0].contains("no speed"),
            "{}",
            notes[0]
        );
    }

    /// "Worst" is the deepest derate — the smallest fraction of the rated speed — and it
    /// is easy to pick the wrong end of the list.
    #[test]
    fn the_worst_offender_is_the_deepest_derate() {
        let capped = vec![
            loaded("mild", 1_000.0, 30_000.0),   // → 80%
            loaded("severe", 1_000.0, 96_000.0), // → 25%
        ];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &capped,
            UserUnitSystem::Metric,
        );
        assert!(
            notes[0].contains("severe") && notes[0].contains("25%"),
            "{}",
            notes[0]
        );

        // Under the floor nothing is scaled, so the tools are ranked by rated speed and
        // the slowest — the furthest from what the machine can do — is the one named.
        let below = vec![
            loaded("mild", 1_000.0, 4_000.0),
            loaded("severe", 1_000.0, 500.0),
        ];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &below,
            UserUnitSystem::Metric,
        );
        assert!(notes[0].contains("severe"), "{}", notes[0]);
    }

    /// A machine that can reach the speed but not move that fast gets its own line, with
    /// the fix named — the axis limits are configuration, and may simply be too low.
    #[test]
    fn an_axis_that_cannot_keep_up_is_reported_separately_from_the_spindle() {
        // 20000 mm/min @ 100000 rpm is 0.2 mm/rev; a 1500 mm/min Z delivers that at
        // 7500 rpm, well inside the spindle range, so the axis is what binds.
        let tools = vec![loaded("0.3mm drill", 20_000.0, 100_000.0)];
        let notes = derate_notes(
            "CNC#1",
            machine(1_000.0, 100_000.0, 5_000.0, 1_500.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert_eq!(notes.len(), 1, "{notes:#?}");
        assert!(
            notes[0].contains("faster than CNC#1 can move"),
            "{}",
            notes[0]
        );
        assert!(
            notes[0].contains("1500mm/min"),
            "names the Z limit: {}",
            notes[0]
        );
        assert!(
            notes[0].contains("the spindle keeps its speed"),
            "must say the spindle was not derated for it: {}",
            notes[0]
        );
        assert!(
            notes[0].contains("cut lighter than rated"),
            "and what that costs: {}",
            notes[0]
        );
        assert!(
            notes[0].contains("Raise the machine's feed limits"),
            "names the fix: {}",
            notes[0]
        );
    }

    /// A high spindle floor no longer changes the answer: the floor is not in the formula,
    /// so this is the same axis-bound case as above and gets the same single note.
    #[test]
    fn a_high_spindle_floor_does_not_add_a_second_complaint() {
        let tools = vec![loaded("0.3mm drill", 20_000.0, 100_000.0)];
        let notes = derate_notes(
            "CNC#1",
            machine(10_000.0, 100_000.0, 5_000.0, 1_500.0),
            &tools,
            UserUnitSystem::Metric,
        );
        assert_eq!(
            notes.len(),
            1,
            "the floor is irrelevant at 100000 rpm: {notes:#?}"
        );
        assert!(
            notes[0].contains("faster than CNC#1 can move"),
            "{}",
            notes[0]
        );
    }

    /// The lateral feed is judged against the **XY** ceiling and nothing else.
    ///
    /// There is no longer a per-tool "is this drilling or routing?" question deciding
    /// which limit applies to one feed: a tool has two rated feeds and each is bound by
    /// its own axis. So a slow Z axis says nothing about a lateral feed the machine can
    /// perfectly well sustain.
    #[test]
    fn the_lateral_feed_is_judged_against_the_xy_ceiling_alone() {
        let router = loaded_router("1.4mm router", 4_400.0, 34_000.0);
        // A crawling Z (1500) with plenty of XY (5000). The lateral 4400 fits XY, and the
        // plunge — a third of it, this tool having no rated one — fits Z easily.
        let notes = derate_notes(
            "CNC#1",
            machine(1_000.0, 100_000.0, 5_000.0, 1_500.0),
            &[router],
            UserUnitSystem::Metric,
        );
        assert!(
            notes.is_empty(),
            "4400 is under the 5000 XY ceiling: {notes:#?}"
        );

        // Lower the XY ceiling below it and the same tool is reported.
        let notes = derate_notes(
            "CNC#1",
            machine(1_000.0, 100_000.0, 2_000.0, 5_000.0),
            &[loaded_router("1.4mm router", 4_400.0, 34_000.0)],
            UserUnitSystem::Metric,
        );
        assert_eq!(notes.len(), 1, "4400 does not fit under 2000: {notes:#?}");
    }

    /// Figures follow the user's units like every other number on the screen.
    #[test]
    fn the_rated_feed_is_shown_in_the_users_units() {
        let tools = vec![loaded("1.0mm drill", 25_400.0, 48_000.0)];
        let notes = derate_notes(
            "CNC#1",
            spindle(5_000.0, 24_000.0),
            &tools,
            UserUnitSystem::Imperial,
        );
        assert!(
            notes[0].contains("1000ipm"),
            "25400 mm/min is 1000 ipm: {}",
            notes[0]
        );
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

    // --- routing tool selection -------------------------------------------

    /// A stock router of the given diameter.
    pub(super) fn router(id: &str, diameter_mm: f64) -> Tool {
        Tool {
            id: id.to_string(),
            composite_name: format!("Router {diameter_mm}mm"),
            name: format!("Router {diameter_mm}mm"),
            kind: "Router".to_string(),
            diameter: Length::from_mm(diameter_mm),
            catalog_diameter: None,
            point_angle: units::Angle::from_degrees(180.0),
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

    /// A toolset with the given tools pinned in slots (empty = nothing fixed).
    pub(super) fn toolset_with_fixed(fixed: &[&str]) -> crate::data::model::ToolsetProfile {
        crate::data::model::ToolsetProfile {
            id: "ts".into(),
            name: "Test".into(),
            description: String::new(),
            generation_policy: ToolsetGenerationPolicy::AllowReload,
            slots: fixed
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    (
                        i as u8 + 1,
                        crate::data::model::state::RackSlot {
                            tool_id: Some((*id).to_string()),
                            locked: true,
                            disabled: false,
                        },
                    )
                })
                .collect(),
            pending_required_fields: Default::default(),
            usable: true,
        }
    }

    fn slot_group(width_mm: f64, length_mm: f64) -> HoleGroup {
        HoleGroup {
            kind: DemandKind::Pth,
            target: Length::from_mm(length_mm),
            minor: Some(Length::from_mm(width_mm)),
            count: 1,
        }
    }

    /// The reported bug: a 1.2 mm cutter cannot enter a 0.4 mm slot, so it must not be
    /// selected for one no matter what the board outline uses.
    #[test]
    fn a_router_wider_than_the_slot_is_never_selected() {
        let tools = vec![router("wide", 1.2)];
        let toolset = toolset_with_fixed(&[]);
        let groups = vec![slot_group(0.4, 3.0)];

        let plan = plan_routers(
            &tools,
            &toolset,
            &groups,
            false,
            Length::from_mm(1e6),
            true,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.for_group(&groups[0]),
            None,
            "1.2mm cutter rejected for a 0.4mm slot"
        );
        assert_eq!(
            plan.unroutable_widths.len(),
            1,
            "and the slot is reported unroutable"
        );
        assert!(
            plan.mandatory_ids().is_empty(),
            "no router is reserved in the rack for it"
        );
    }

    /// Among cutters that fit, the largest wins: fewest passes and the stiffest tool,
    /// with one exactly the slot width milling it in a single pass.
    #[test]
    fn the_slot_router_is_the_largest_that_fits() {
        let tools = vec![
            router("tiny", 0.2),
            router("exact", 0.4),
            router("wide", 1.2),
        ];
        let toolset = toolset_with_fixed(&[]);
        let groups = vec![slot_group(0.4, 3.0)];

        let plan = plan_routers(
            &tools,
            &toolset,
            &groups,
            false,
            Length::from_mm(1e6),
            true,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(plan.for_group(&groups[0]), Some("exact"));
        assert!(plan.unroutable_widths.is_empty());
    }

    /// A toolset-pinned router that fits beats a larger in-stock one — it is already in
    /// the rack, so it costs no slot.
    #[test]
    fn a_fixed_router_that_fits_beats_a_larger_stock_one() {
        let tools = vec![router("fixed", 0.3), router("bigger", 0.4)];
        let toolset = toolset_with_fixed(&["fixed"]);
        let groups = vec![slot_group(0.4, 3.0)];

        let plan = plan_routers(
            &tools,
            &toolset,
            &groups,
            false,
            Length::from_mm(1e6),
            true,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(plan.for_group(&groups[0]), Some("fixed"));
    }

    /// **The kerf IS the cutter, matched exactly.**
    ///
    /// A single-pass route removes exactly one cutter width, so a 2 mm kerf is a 2 mm
    /// router. It used to be "the smallest router in stock", which loaded a 0.4 mm cutter
    /// to cut an outline a 2 mm cutter clears in a fraction of the time, with nothing the
    /// operator could set to say otherwise.
    #[test]
    fn the_outline_router_is_the_kerf_exactly() {
        let tools = vec![router("a", 0.8), router("b", 1.4), router("c", 2.0)];
        let toolset = toolset_with_fixed(&[]);

        let outline = |kerf_mm: f64| {
            plan_routers(
                &tools,
                &toolset,
                &[],
                true,
                Length::from_mm(kerf_mm),
                false,
                CutoutRouting { contours: &[], relieve_corners: false },
            )
            .outline
        };

        assert_eq!(outline(2.0).as_deref(), Some("c"));
        assert_eq!(outline(1.4).as_deref(), Some("b"));
        assert_eq!(outline(0.8).as_deref(), Some("a"));
    }

    /// **A kerf with no cutter is a refusal, not a substitution.**
    ///
    /// Cutting a 2 mm channel with the 1.4 mm bit that happens to be in the drawer does
    /// not produce a slightly worse board, it produces a different one — so the step fails
    /// and the operator is told which cutter to stock. The same rule locating pins follow.
    #[test]
    fn a_kerf_with_no_matching_router_is_refused_rather_than_approximated() {
        let tools = vec![router("narrower", 1.4), router("wider", 3.0)];
        let plan = plan_routers(
            &tools,
            &toolset_with_fixed(&[]),
            &[],
            true,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.outline, None,
            "neither the nearest smaller nor the nearest larger stands in for a 2mm kerf"
        );
    }

    /// **Being pinned in the toolset breaks a tie; it does not win an argument.**
    ///
    /// A pinned router costs no rack slot, which is worth something — but not worth
    /// cutting a different channel than the one the profile asked for. The old rule
    /// returned the first fixed router outright, so pinning a 0.8 quietly overrode a
    /// request for a 2 mm kerf with a 2 mm cutter sitting in stock.
    #[test]
    fn a_pinned_router_does_not_override_the_requested_kerf() {
        let tools = vec![router("pinned-narrow", 0.8), router("stocked-exact", 2.0)];
        let plan = plan_routers(
            &tools,
            &toolset_with_fixed(&["pinned-narrow"]),
            &[],
            true,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.outline.as_deref(),
            Some("stocked-exact"),
            "the requested kerf wins over the free rack slot"
        );

        // Among cutters of the *same* diameter, the pinned one is the better choice.
        let tools = vec![router("loose", 2.0), router("pinned", 2.0)];
        let plan = plan_routers(
            &tools,
            &toolset_with_fixed(&["pinned"]),
            &[],
            true,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.outline.as_deref(),
            Some("pinned"),
            "a tie goes to the free slot"
        );
    }

    /// The outline cutter is a **required** tool of the step: chosen outside the assigner,
    /// so nothing else would reserve it a slot, and a step that cannot load it cannot cut
    /// the board free.
    #[test]
    fn the_kerf_cutter_is_reserved_a_rack_slot() {
        let tools = vec![router("kerf", 2.0), router("slot", 0.4)];
        let groups = vec![slot_group(1.0, 4.0)];
        let plan = plan_routers(
            &tools,
            &toolset_with_fixed(&[]),
            &groups,
            true,
            Length::from_mm(2.0),
            true,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.for_group(&groups[0]),
            Some("slot"),
            "2.0mm will not enter a 1.0mm slot"
        );
        assert_eq!(
            plan.mandatory_ids(),
            vec!["kerf".to_string(), "slot".to_string()],
            "both the kerf cutter and the slot cutter must be in the rack"
        );
    }

    /// Builds a step that machines a face, with everything else irrelevant to the test.
    fn step_on(name: &str, machines_back: bool) -> StepRaw {
        StepRaw {
            name: name.into(),
            operations: vec!["drill_pth".into()],
            cnc_id: Some(uuid::Uuid::now_v7()),
            fixture_id: Some(uuid::Uuid::now_v7()),
            toolset_id: Some(uuid::Uuid::now_v7()),
            drill: Default::default(),
            route_board: Default::default(),
            route_cutouts: Default::default(),
            engrave_copper: Default::default(),
            machines_back,
            pin_diameter: None,
        }
    }

    /// A step that drills locating pins, on the face given.
    fn pin_step(name: &str, machines_back: bool) -> StepRaw {
        StepRaw {
            operations: vec!["drill_locating_pins".into()],
            pin_diameter: Some(Length::from_mm(3.2)),
            ..step_on(name, machines_back)
        }
    }

    /// A profile that machines one face throughout needs no pins at all — which is the
    /// overwhelmingly common job, and must stay frictionless.
    #[test]
    fn a_single_sided_job_needs_no_locating_pins() {
        assert!(
            locating_pin_faults(&[]).is_empty(),
            "no steps, nothing to fault"
        );
        assert!(
            locating_pin_faults(&[step_on("Drill", false), step_on("Cut out", false)]).is_empty(),
            "an all-front profile generates"
        );
        assert!(
            locating_pin_faults(&[step_on("A", true), step_on("B", true)]).is_empty(),
            "and so does an all-back one — the board is mounted that way, not turned over"
        );
    }

    /// **The rule this whole feature exists to enforce.** Two steps on opposite faces with
    /// nothing registering the board between them is a board lifted off the fixture and put
    /// back by eye — and the program that follows is exact to a micron and lands wherever
    /// the operator happened to put it.
    #[test]
    fn changing_faces_without_pins_is_refused() {
        let faults =
            locating_pin_faults(&[step_on("Drill front", false), step_on("Drill back", true)]);
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(
            faults[0].contains("steps 1 and 2 machine different faces"),
            "names the pair: {}",
            faults[0]
        );
        assert!(
            faults[0].contains("add one first"),
            "says what to do: {}",
            faults[0]
        );

        assert!(
            locating_pin_faults(&[
                pin_step("Pins", false),
                step_on("Drill front", false),
                step_on("Drill back", true),
            ])
            .is_empty(),
            "with pins drilled first, the same profile is fine"
        );
    }

    /// The pair named is the *boundary* — where the board is turned over — not merely two
    /// adjacent steps. A front/front/back profile changes face between steps 1 and 3.
    #[test]
    fn the_reported_pair_is_where_the_board_turns_over() {
        let faults = locating_pin_faults(&[
            step_on("Drill", false),
            step_on("Route", false),
            step_on("Back face", true),
        ]);
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(faults[0].contains("steps 1 and 3"), "{}", faults[0]);
    }

    /// Pins drilled *after* something else has already cut the board register it against
    /// holes made after the fact, which registers nothing. They have to come first.
    #[test]
    fn pins_must_be_the_first_step() {
        let faults = locating_pin_faults(&[step_on("Drill", false), pin_step("Pins", false)]);
        assert!(
            faults
                .iter()
                .any(|f| f.contains("step 2 'Pins'") && f.contains("not the first step")),
            "{faults:?}"
        );
    }

    /// Pins on the back mean the board was already turned over — before it had anything to
    /// be turned over against. Only reachable by hand-editing, because the editor hides the
    /// face control on a pins step.
    #[test]
    fn pins_may_not_be_drilled_from_the_back() {
        let faults = locating_pin_faults(&[pin_step("Pins", true)]);
        assert!(
            faults
                .iter()
                .any(|f| f.contains("back face") && f.contains("set its board face to front")),
            "{faults:?}"
        );
    }

    /// A pin step that comes *after* the face change has not registered the steps that
    /// were cut before it, so its presence is not enough — its position is the point.
    #[test]
    fn pins_after_the_face_change_do_not_register_what_came_before() {
        let faults = locating_pin_faults(&[
            step_on("Drill front", false),
            pin_step("Pins", true),
            step_on("Drill back", true),
        ]);
        assert!(
            faults.iter().any(|f| f.contains("machine different faces")),
            "the face change is still unregistered: {faults:?}"
        );
    }

    /// Two steps cutting the same feature on the same side must stop generation.
    ///
    /// The editor disables the checkbox that would do this, so a profile reaching the
    /// gate with a clash was hand-edited or imported. It cannot be allowed through: the
    /// second program drives a tool back through holes that are already there, into a
    /// board the first program may have released from its tabs.
    #[test]
    fn an_operation_claimed_by_two_steps_on_one_side_is_refused() {
        let step_doing = |name: &str, bottom: bool, ops: &[&str]| StepRaw {
            operations: ops.iter().map(|op| op.to_string()).collect(),
            ..step_on(name, bottom)
        };

        assert_eq!(
            duplicate_operations_reason(&[]),
            None,
            "no steps, nothing to clash"
        );
        assert_eq!(
            duplicate_operations_reason(&[
                step_doing("Drill", false, &["drill_pth", "drill_npth"]),
                step_doing("Cut out", false, &["route_board"]),
            ]),
            None,
            "a profile that splits its work generates"
        );

        let reason = duplicate_operations_reason(&[
            step_doing("Drill", false, &["drill_pth"]),
            step_doing("Drill again", false, &["drill_pth"]),
        ])
        .expect("the same holes drilled twice must be refused");
        assert!(
            reason.contains("Drill plated holes"),
            "names the operation: {reason}"
        );
        assert!(
            reason.contains("'Drill'") && reason.contains("'Drill again'"),
            "names both steps: {reason}"
        );
    }

    /// Milling the front and then the back is the two-sided workflow, not a mistake —
    /// the rule counts the faces apart.
    #[test]
    fn the_same_operation_on_opposite_faces_is_allowed_through() {
        let step_doing = |name: &str, back: bool, ops: &[&str]| StepRaw {
            operations: ops.iter().map(|op| op.to_string()).collect(),
            ..step_on(name, back)
        };

        assert_eq!(
            duplicate_operations_reason(&[
                step_doing("Mill the front", false, &["mill_board"]),
                step_doing("Mill the back", true, &["mill_board"]),
            ]),
            None,
        );
    }

    /// A step with no profile chosen is refused by name, not defaulted. This is the whole
    /// point of allowing "none": it makes the step unrunnable rather than silently
    /// planning against a machine and fixture the operator never picked.
    #[test]
    fn a_step_missing_a_profile_is_refused_and_says_which() {
        let step = |cnc: bool, fixture: bool, toolset: bool| StepRaw {
            name: "Step".into(),
            operations: vec!["drill_pth".into()],
            cnc_id: cnc.then(uuid::Uuid::now_v7),
            fixture_id: fixture.then(uuid::Uuid::now_v7),
            toolset_id: toolset.then(uuid::Uuid::now_v7),
            drill: Default::default(),
            route_board: Default::default(),
            route_cutouts: Default::default(),
            engrave_copper: Default::default(),
            machines_back: false,
            pin_diameter: None,
        };
        assert_eq!(
            missing_bindings(&step(true, true, true)),
            None,
            "all three set"
        );
        assert_eq!(
            missing_bindings(&step(false, true, true)).as_deref(),
            Some("This step has no CNC profile selected.")
        );
        assert_eq!(
            missing_bindings(&step(true, false, true)).as_deref(),
            Some("This step has no fixture profile selected.")
        );
        assert_eq!(
            missing_bindings(&step(false, false, false)).as_deref(),
            Some("This step has no CNC, no fixture, no toolset profile selected."),
            "a fresh step names all three at once rather than one per attempt"
        );
    }

    #[test]
    fn slot_routers_are_only_resolved_when_the_strategy_routes() {
        let tools = vec![router("fits", 0.4)];
        let toolset = toolset_with_fixed(&[]);
        let groups = vec![slot_group(0.4, 3.0)];

        let plan = plan_routers(
            &tools,
            &toolset,
            &groups,
            false,
            Length::from_mm(1e6),
            false,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.for_group(&groups[0]),
            None,
            "drill-only strategy reserves no router"
        );
        assert!(
            plan.unroutable_widths.is_empty(),
            "and reports nothing unroutable"
        );

        let round = HoleGroup {
            kind: DemandKind::Pth,
            target: Length::from_mm(0.8),
            minor: None,
            count: 1,
        };
        let plan = plan_routers(
            &tools,
            &toolset,
            &groups,
            false,
            Length::from_mm(1e6),
            true,
            CutoutRouting { contours: &[], relieve_corners: false },
        );
        assert_eq!(
            plan.for_group(&round),
            None,
            "a round hole is drilled, not milled"
        );
    }

    /// A step as the rack scheduler takes it.
    fn rack_step(step_index: usize, tools: &[&str]) -> RackStepInput {
        RackStepInput {
            step_index,
            tool_ids: tools.iter().map(|id| id.to_string()).collect(),
        }
    }

    /// A built schedule's column for `step_index`, with nothing left unplaced.
    fn column(step_index: usize) -> RackStepColumn {
        RackStepColumn {
            step_index,
            unplaced: vec![],
        }
    }

    #[test]
    fn cross_step_schedule_keeps_reused_tools_in_the_same_slot() {
        use std::collections::BTreeSet;
        // Spare slots T2/T3/T4; a tool "fix" is pinned (fixed) elsewhere.
        let fixed: BTreeSet<String> = ["fix".to_string()].into_iter().collect();
        let steps = vec![
            rack_step(0, &["fix", "A", "B"]),
            rack_step(1, &["fix", "B", "C"]),
        ];
        let snaps = schedule_spare_slots(&[2, 3, 4], &fixed, &steps);

        // Step 1 loads A→T2 and B→T3 (both changed).
        let (state1, changed1) = (&snaps[0].state, &snaps[0].changed);
        assert_eq!(state1.get(&2), Some(&"A".to_string()));
        assert_eq!(state1.get(&3), Some(&"B".to_string()));
        assert_eq!(changed1, &BTreeSet::from([2, 3]));

        // Step 2: B keeps T3 (the optimisation), C takes the empty T4; A idles in T2.
        let (state2, changed2) = (&snaps[1].state, &snaps[1].changed);
        assert_eq!(
            state2.get(&3),
            Some(&"B".to_string()),
            "B stays put across steps"
        );
        assert_eq!(state2.get(&4), Some(&"C".to_string()));
        assert_eq!(
            state2.get(&2),
            Some(&"A".to_string()),
            "idle tool is left loaded"
        );
        assert_eq!(
            changed2,
            &BTreeSet::from([4]),
            "only the new tool's slot changes"
        );
    }

    /// Columns exist only for steps that resolved *and* loaded tools, so a column's
    /// position is not its step index. Projecting by position would show step 3 the rack
    /// belonging to whichever step happened to be third in the schedule.
    #[test]
    fn the_rack_is_projected_by_step_index_not_by_column_position() {
        let schedule = RackSchedule {
            cnc_name: "Router".into(),
            clipped_slots: 0,
            // Steps 0 and 2 resolved; step 1 loaded nothing and has no column.
            steps: vec![column(0), column(2)],
            slots: vec![RackSlotSchedule {
                slot: "T1".to_string(),
                cells: vec![
                    RackCell {
                        tool: Some("drill".into()),
                        status: SlotChange::Load,
                    },
                    RackCell {
                        tool: Some("router".into()),
                        status: SlotChange::Kept,
                    },
                ],
            }],
        };

        let step0 = schedule.step_view(0).expect("step 0 has a column");
        assert_eq!(step0.slots[0].tool.as_deref(), Some("drill"));
        assert_eq!(step0.slots[0].status, SlotChange::Load);

        // The *second* column belongs to step 2, not step 1.
        assert!(
            schedule.step_view(1).is_none(),
            "a step with no column has no rack"
        );
        let step2 = schedule.step_view(2).expect("step 2 has a column");
        assert_eq!(step2.slots[0].tool.as_deref(), Some("router"));
        assert_eq!(
            step2.slots[0].status,
            SlotChange::Kept,
            "carried over from an earlier step"
        );

        assert!(
            schedule.step_view(9).is_none(),
            "a step beyond the profile has none"
        );
    }

    /// A rack is one machine's carousel, so a step is looked up in the rack it runs
    /// against and nowhere else. Two CNCs sharing one toolset was the case that broke:
    /// the job had a single schedule, so the second machine's step read the first
    /// machine's rack — or, when the second machine had no changer at all, read nothing
    /// and the view fell back to the board.
    #[test]
    fn a_step_reads_the_rack_of_its_own_machine() {
        let plan = ToolingPlan {
            steps: vec![],
            note: None,
            racks: vec![
                RackSchedule {
                    cnc_name: "Driller".into(),
                    clipped_slots: 0,
                    steps: vec![column(0)],
                    slots: vec![RackSlotSchedule {
                        slot: "T1".into(),
                        cells: vec![RackCell {
                            tool: Some("drill".into()),
                            status: SlotChange::Load,
                        }],
                    }],
                },
                RackSchedule {
                    cnc_name: "Router".into(),
                    clipped_slots: 0,
                    steps: vec![column(1)],
                    slots: vec![RackSlotSchedule {
                        slot: "T1".into(),
                        cells: vec![RackCell {
                            tool: Some("cutter".into()),
                            status: SlotChange::Load,
                        }],
                    }],
                },
            ],
        };

        let first = plan.rack_for_step(0).expect("step 0 runs on the driller");
        assert_eq!(first.cnc_name, "Driller");
        assert_eq!(first.slots[0].tool.as_deref(), Some("drill"));

        let second = plan.rack_for_step(1).expect("step 1 runs on the router");
        assert_eq!(second.cnc_name, "Router");
        assert_eq!(
            second.slots[0].tool.as_deref(),
            Some("cutter"),
            "the second machine's rack, not the first machine's"
        );
        assert_eq!(
            second.step_count, 1,
            "one step on this rack, so nothing can have been kept from another"
        );

        assert!(
            plan.rack_for_step(2).is_none(),
            "a step on no rack (manual tool changes) has none to show"
        );
    }

    #[test]
    fn overflow_reuses_a_slot_by_evicting_a_tool_not_needed_this_step() {
        use std::collections::BTreeSet;
        // Only one spare slot: step 2's tool must evict step 1's (no longer needed).
        let snaps = schedule_spare_slots(
            &[1],
            &BTreeSet::new(),
            &[rack_step(0, &["A"]), rack_step(1, &["B"])],
        );
        assert_eq!(snaps[0].state.get(&1), Some(&"A".to_string()));
        assert_eq!(
            snaps[1].state.get(&1),
            Some(&"B".to_string()),
            "B evicts the idle A"
        );
        assert!(snaps[1].changed.contains(&1), "the reload counts as a change");
        assert!(
            snaps[1].unplaced.is_empty(),
            "an eviction is not an overflow — the tool did get a slot"
        );
    }

    /// A step needing more tools at once than the rack has slots cannot be scheduled into
    /// it. The tools are named rather than quietly left out, because a rack listing that
    /// omits a tool the program calls reads as a complete rack.
    #[test]
    fn a_tool_with_no_slot_left_is_reported_rather_than_dropped() {
        use std::collections::BTreeSet;
        let snaps = schedule_spare_slots(
            &[1, 2],
            &BTreeSet::new(),
            &[rack_step(0, &["A", "B", "C"])],
        );
        assert_eq!(snaps[0].state.len(), 2, "both slots are filled");
        assert_eq!(
            snaps[0].unplaced,
            vec!["C".to_string()],
            "the third tool has nowhere to go and says so"
        );
    }

    #[test]
    fn detects_oblong_and_keeps_major_and_minor_axes() {
        let holes = vec![hole(HoleKind::PadNpth, 2.0, 4.0)];
        let groups = collect_hole_groups(&holes, false, true);
        assert_eq!(groups.len(), 1);
        assert_eq!(micron(groups[0].target), 4000, "major axis is the target");
        assert_eq!(
            groups[0].minor.map(micron),
            Some(2000),
            "minor axis retained"
        );
        assert_eq!(groups[0].kind, DemandKind::Npth);
    }

    #[test]
    fn filters_holes_by_enabled_operation() {
        let holes = vec![
            hole(HoleKind::PadPth, 0.8, 0.8),
            hole(HoleKind::PadNpth, 3.0, 3.0),
        ];
        // Only PTH enabled → the NPTH hole is excluded from the demand.
        let groups = collect_hole_groups(&holes, true, false);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DemandKind::Pth);
    }
}

#[cfg(test)]
mod cutout_router_tests {
    use super::*;
    use crate::data::model::ToolsetProfile;

    fn nm(mm: f64) -> i64 {
        (mm * 1e6) as i64
    }

    /// A rectangular opening `w x h` mm, as the stitcher would hand it over.
    fn opening(w: f64, h: f64) -> pcb::Contour {
        let pts = vec![(0, 0), (nm(w), 0), (nm(w), nm(h)), (0, nm(h))];
        let segments = (0..4)
            .map(|i| pcb::Segment::Line {
                start: pts[i],
                end: pts[(i + 1) % 4],
            })
            .collect();
        pcb::Contour {
            points: pts,
            segments,
            is_hole: true,
        }
    }

    fn drill(id: &str, diameter_mm: f64) -> Tool {
        let mut t = super::tests::router(id, diameter_mm);
        t.kind = "Drillbit".to_string();
        t.name = format!("Drill {diameter_mm}mm");
        t
    }

    fn empty_toolset() -> ToolsetProfile {
        super::tests::toolset_with_fixed(&[])
    }

    /// The cutter is chosen to fit the opening, not to match a requested size.
    ///
    /// This is the whole reason interior openings are their own operation. A 1.5mm slot
    /// cut under `route_board` is measured against the edge kerf, and a 2mm kerf reports
    /// it as vanishing rather than reaching for the 1.4mm router sitting in stock. The
    /// widest cutter that fits is preferred because the wall pass is one lap whatever the
    /// diameter — a wider cutter buys stiffness, never fewer passes.
    #[test]
    fn each_opening_takes_the_widest_router_that_fits_it() {
        let tools = vec![
            super::tests::router("r3", 3.0),
            super::tests::router("r1_4", 1.4),
            super::tests::router("r0_8", 0.8),
        ];
        let toolset = empty_toolset();

        let wide = opening(8.0, 5.0);
        let narrow = opening(9.5, 1.5);
        let plan = plan_routers(
            &tools,
            &toolset,
            &[],
            false,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[(0, &wide), (1, &narrow)], relieve_corners: false },
        );

        assert_eq!(
            plan.cutouts[&0].0, "r3",
            "5mm of width takes the 3mm cutter"
        );
        assert_eq!(
            plan.cutouts[&1].0, "r1_4",
            "1.5mm of width takes the 1.4mm cutter, not the 3mm one"
        );
        assert!(plan.uncuttable_cutouts.is_empty());
    }

    /// An opening no router can enter is reported, not quietly skipped.
    ///
    /// Cutting it with something too wide would take out the board either side of it.
    #[test]
    fn an_opening_narrower_than_every_router_is_reported_uncuttable() {
        let tools = vec![super::tests::router("r2", 2.0)];
        let hairline = opening(9.0, 0.5);
        let plan = plan_routers(
            &tools,
            &empty_toolset(),
            &[],
            false,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[(0, &hairline)], relieve_corners: false },
        );

        assert!(plan.cutouts.is_empty());
        assert_eq!(plan.uncuttable_cutouts, vec![0]);
    }

    /// Corner relief drills must be **reserved**, or they can never be used.
    ///
    /// A step that only cuts openings drills nothing else, so its rack is built from the
    /// routers alone. Choosing the relief drill at plan time from "whatever is already
    /// racked" — the rule mouse bites follow — would therefore skip every corner on
    /// exactly the step the feature exists for, with a drill in stock the whole time.
    #[test]
    fn corner_relief_drills_are_reserved_a_slot() {
        let tools = vec![
            super::tests::router("r3", 3.0),
            drill("d0_9", 0.9),
            drill("d0_1", 0.1),
        ];
        let square = opening(8.0, 8.0);
        let plan = plan_routers(
            &tools,
            &empty_toolset(),
            &[],
            false,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[(0, &square)], relieve_corners: true },
        );

        // 3mm cutter, 90 degree corners: ideal = 3 * (1 - sin45) = 0.879mm, and the band
        // reaches down to 0.879 / (1 + sin45) = 0.515mm. The 0.9mm drill is over the
        // ideal; the 0.1mm one is under the merge floor and would leave an uncut web.
        let chosen = &plan.corner_drills[&0];
        assert_eq!(chosen.len(), 4, "four corners");
        assert!(
            chosen.iter().all(|d| d.is_none()),
            "neither drill is inside the band, so no corner is relieved: {chosen:?}"
        );

        // With one in the band, it is chosen *and* reserved.
        let tools = vec![super::tests::router("r3", 3.0), drill("d0_8", 0.8)];
        let plan = plan_routers(
            &tools,
            &empty_toolset(),
            &[],
            false,
            Length::from_mm(2.0),
            false,
            CutoutRouting { contours: &[(0, &square)], relieve_corners: true },
        );
        assert!(plan.corner_drills[&0]
            .iter()
            .all(|d| d.as_deref() == Some("d0_8")));
        assert!(
            plan.mandatory_ids().contains(&"d0_8".to_string()),
            "the relief drill must hold a rack slot: {:?}",
            plan.mandatory_ids()
        );
    }

    /// The relief band, at the one angle with a value anyone can check by eye.
    ///
    /// A drill below the minimum does not reach the routed pocket, so instead of a
    /// sharper corner it leaves a ring of uncut material and a loose chip.
    #[test]
    fn the_corner_relief_band_runs_from_the_merge_diameter_to_the_ideal() {
        let (min, ideal) = corner_relief_band(2.0, std::f64::consts::FRAC_PI_2).unwrap();
        assert!(
            (ideal - 0.5857864).abs() < 1e-6,
            "2mm cutter, 90 degrees: {ideal}"
        );
        assert!((min - ideal / (1.0 + 0.5_f64.sqrt())).abs() < 1e-9);
        // A straight edge is not a corner and wants no drill.
        assert!(corner_relief_band(2.0, std::f64::consts::PI).is_none());
    }
}

#[cfg(test)]
mod engraver_tests {
    use super::*;

    /// A V-bit: a tip flat of `tip_mm` on a cone of `angle_deg`.
    fn vbit(id: &str, tip_mm: f64, angle_deg: f64) -> Tool {
        // The diameter *is* the tip — the narrowest channel the bit can cut.
        let mut tool = super::tests::router(id, tip_mm);
        tool.kind = "V-bit".to_string();
        tool.name = format!("V-bit {tip_mm}mm/{angle_deg}deg");
        tool.point_angle = units::Angle::from_degrees(angle_deg);
        tool
    }

    /// At one channel width a broader tip sits shallower, and a shallower cut is far less
    /// sensitive to how flat the board is: on a steep cone a tenth of a millimetre of bow
    /// swings the trace width, on a broad one it barely moves. Width *is* the product
    /// here, so that sensitivity is the thing being chosen against.
    #[test]
    fn the_widest_tip_that_still_reaches_the_width_is_chosen() {
        let tools = vec![vbit("fine", 0.1, 30.0), vbit("broad", 0.2, 30.0)];
        let (id, depth) = pick_engraver(&tools, &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.4))
            .expect("both bits can cut 0.4mm");

        assert_eq!(id, "broad");
        // 0.2mm of tip leaves 0.2mm to open out, at tan(15°) per side.
        let expected = 0.1 / (15.0_f64.to_radians().tan());
        assert!((depth.as_mm() - expected).abs() < 1e-6, "depth was {}", depth.as_mm());
    }

    /// A bit whose tip is already broader than the channel cannot cut it at any depth —
    /// the tip is the narrowest thing it makes. Substituting the nearest would hand back a
    /// cut wider than asked for, straight through a neighbouring net.
    #[test]
    fn a_tip_broader_than_the_channel_is_refused() {
        let tools = vec![vbit("broad", 0.5, 30.0)];
        assert!(
            pick_engraver(&tools, &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.3))
                .is_none()
        );
    }

    /// Depth here is arrived at by arithmetic from a width, and arithmetic will happily
    /// ask for less than the tool is rated to hold. A cut shallower than its rating is a
    /// cut whose width nobody can promise.
    #[test]
    fn a_depth_under_the_bit_rating_is_refused() {
        let mut shallow = vbit("rated", 0.1, 30.0);
        shallow.z_min_depth = Some(Length::from_mm(0.5));
        // 0.2mm across needs ~0.19mm of depth — well under the 0.5mm rating.
        assert!(
            pick_engraver(&[shallow], &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.2))
                .is_none()
        );
    }

    /// Between two bits with the same tip, the shallower cone is the one whose width moves
    /// least when the board is not perfectly flat: at 90° a tenth of a millimetre of bow
    /// costs two tenths of trace width, at 15° it costs two hundredths. Left to the tool
    /// id this would come out arbitrary, and the generic catalogue ships three bits
    /// sharing a 0.2 mm tip — so it would come out arbitrary in practice, not in theory.
    #[test]
    fn among_equal_tips_the_shallowest_cone_wins() {
        let tools = vec![vbit("steep", 0.2, 60.0), vbit("shallow", 0.2, 30.0)];
        let (id, depth) = pick_engraver(&tools, &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.4))
            .expect("both can cut 0.4mm");

        assert_eq!(id, "shallow");
        assert!(depth.as_mm() > 0.3, "and it gets there by cutting deeper: {}", depth.as_mm());
    }

    /// A width no bit can reach yields nothing, and it is on that `None` that the step is
    /// refused rather than planned without its engraving. A program that drilled and
    /// routed and quietly did not isolate would look finished to whoever ran it.
    #[test]
    fn a_width_no_bit_can_reach_yields_nothing_to_engrave_with() {
        let tools = vec![vbit("coarse", 0.5, 30.0), vbit("coarser", 0.8, 30.0)];
        assert!(
            pick_engraver(&tools, &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.3))
                .is_none(),
            "every tip is wider than the channel asked for"
        );
    }

    /// A router has one width whatever it is asked for, so letting one into this list
    /// would cut a channel of its own diameter and call it isolation.
    #[test]
    fn a_router_is_never_picked_to_engrave() {
        let tools = vec![super::tests::router("r", 0.2)];
        assert!(
            pick_engraver(&tools, &super::tests::toolset_with_fixed(&[]), Length::from_mm(0.4))
                .is_none()
        );
    }

    /// A bit already pinned in the toolset costs no rack slot, so it wins a tie — the same
    /// preference every other tool pick here makes.
    #[test]
    fn a_bit_already_in_the_rack_beats_an_equal_one_in_stock() {
        let tools = vec![vbit("stock", 0.1, 30.0), vbit("racked", 0.1, 30.0)];
        let (id, _) = pick_engraver(
            &tools,
            &super::tests::toolset_with_fixed(&["racked"]),
            Length::from_mm(0.4),
        )
        .expect("either would do");
        assert_eq!(id, "racked");
    }
}
