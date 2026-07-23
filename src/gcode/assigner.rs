//! Tool selection — the "Tools / Assigner" module (Specification.md §8.7).
//!
//! Given the hole demand for **one machining step**, the step's stock tools, the
//! matching allowances, the rack, and the setup geometry, this picks the tool for
//! every hole and the rack layout that runs them. It is a pure, deterministic
//! function with no I/O and no application context, so it unit-tests in isolation
//! (architecture.md §Tools). It is **not** yet wired into `run_generation`; an
//! app-side adapter (later phase) builds these inputs from `BoardSnapshot` + stock
//! + toolset + CNC + the step's drill config.
//!
//! The pipeline, per §8.7:
//!   1. build per-hole candidate tools (drills within the allowance window; routers
//!      as a non-preferred fallback), gated by **Z-feasibility** (§2½ of the plan);
//!   2. score-rank and pick the best per hole (drilling dominant, then size fit,
//!      stock preference, reuse);
//!   3. **shrink** the tool set to the rack capacity by minimum-regret removal;
//!   4. fall back to routing (optional pilot) where no drill is feasible;
//!   5. fail hard, or degrade with a warning, when the rack cannot hold the set.
//!
//! All diameter/fit comparisons are quantised to **1 µm** so tools within a micron
//! are treated as the same size (deterministic tie-breaks).

#![allow(dead_code)] // Consumed by tests today; wired into generation in a later phase.

use std::collections::{BTreeMap, BTreeSet};

use units::Length;

use crate::data::model::tool_core::ToolKind;
use crate::data::model::{Tool, ToolPreference, ToolStatus};

/// Float slop for millimetre range comparisons (1 nm) — well below the 1 µm domain
/// precision, used only to keep boundary inclusions from tripping on FP noise.
const EPS_MM: f64 = 1e-6;
/// Two scores within this are treated as a tie (then broken by diameter, then order).
const SCORE_EPS: f64 = 1e-6;
/// Extra regret charged when a shrink removal forces a hole from drilling to routing.
const DRILL_TO_ROUTE_PENALTY: f64 = 500.0;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// Which operation produced a hole in the demand set (drives diagnostics only in
/// Phase 1; the adapter maps board-hole kinds + the step op onto these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemandKind {
    Pth,
    Npth,
    Locating,
    CornerRelief,
}

/// One required feature, normalised for tool matching. `target` is the final
/// (plating-compensated) diameter to achieve; for an oblong hole `minor_axis` is
/// the width that governs drill suitability. `routable` is whether routing this
/// feature is geometrically permissible.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleDemand {
    pub id: String,
    pub kind: DemandKind,
    pub target: Length,
    pub minor_axis: Option<Length>,
    pub plated: bool,
    pub routable: bool,
}

impl HoleDemand {
    /// The diameter a drill must match — the minor axis for an oblong hole,
    /// otherwise the round target.
    fn match_length(&self) -> Length {
        self.minor_axis.unwrap_or(self.target)
    }
}

/// A tolerance band for substituting a stock tool for a required size: a relative
/// fraction of the hole, capped by an absolute maximum. Mirrors the
/// `machining.yaml` `$defs/allowance` shape.
#[derive(Clone, Copy, Debug)]
pub struct Allowance {
    /// Fraction of the hole diameter, e.g. `0.08` for 8 %.
    pub relative: f64,
    pub max: Length,
}

impl Allowance {
    /// The effective allowance for a given diameter: `min(relative × d, max)`.
    fn effective_mm(&self, diameter_mm: f64) -> f64 {
        (self.relative * diameter_mm).min(self.max.as_mm())
    }
}

/// The setup geometry driving Z-feasibility (§2½): board thickness `T`, the
/// fixture's below-board clearance `C`, and the breakthrough margin `m`.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    pub board_thickness: Length,
    pub bed_clearance: Length,
    pub breakthrough_margin: Length,
}

/// Scoring weights. `strategy` is dominant so a drill always beats a router unless
/// no feasible drill exists.
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    pub strategy: f64,
    pub fit: f64,
    pub pref: f64,
    pub reuse: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { strategy: 1_000_000.0, fit: 100.0, pref: 1.0, reuse: 0.01 }
    }
}

/// Per-step tool-selection configuration (from the step's `drill_pth`/`drill_npth`
/// `holes` config).
#[derive(Clone, Debug)]
pub struct AssignConfig {
    pub allow_routing_holes: bool,
    pub drill_first: bool,
    pub pilot: bool,
    pub oversize: Allowance,
    pub undersize: Allowance,
    pub weights: Weights,
}

/// What happens when the required tools exceed the rack (from the toolset's
/// `generation_policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowPolicy {
    FixedToolset,
    AllowReload,
    AllowHybrid,
}

/// The rack: `capacity` usable slots (K), the `fixed` slots pinned to a tool, the
/// `spare_slots` (physical slot numbers) auto-selected tools may occupy, and
/// `mandatory` tool ids that must be present and never removed by shrink (the
/// routing set `R_project`). Fixed tools are implicitly mandatory too. Slots that
/// are neither fixed nor spare (e.g. do-not-use) simply do not appear here, so they
/// are never assigned.
#[derive(Clone, Debug)]
pub struct RackSpec {
    pub capacity: usize,
    pub fixed: Vec<(u8, String)>,
    pub spare_slots: Vec<u8>,
    pub mandatory: Vec<String>,
    pub policy: OverflowPolicy,
}

// ---------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    Drill,
    Route,
}

/// The tool chosen for one hole, with the explainability fields §8.7 requires.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleAssignment {
    pub hole_id: String,
    pub tool_id: String,
    pub strategy: Strategy,
    /// Pilot drill for a routed hole, when enabled and available.
    pub pilot_tool_id: Option<String>,
    /// Absolute size error |tool − target| in micrometres.
    pub fit_error_um: i64,
    /// Plunge depth past the top surface (`T + Lp + m`).
    pub z_bottom: Length,
    pub changed_by_shrink: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotAssignment {
    pub slot: u8,
    pub tool_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Removal {
    pub tool_id: String,
    pub reason: String,
    pub regret: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolAssignment {
    pub holes: Vec<HoleAssignment>,
    pub rack: Vec<SlotAssignment>,
    pub removed: Vec<Removal>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a hole has no usable tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultReason {
    /// No drill falls in the size window and routing is off / no router fits.
    NoSizeMatch,
    /// A size-matching drill exists but fails depth/fixture feasibility (§2½).
    DepthInfeasible,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoleFault {
    pub hole_id: String,
    pub kind: DemandKind,
    pub target_um: i64,
    pub reason: FaultReason,
    /// Closest in-stock drills, for actionable diagnostics.
    pub nearest: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AssignError {
    /// One or more holes cannot be covered by any feasible tool.
    UncoverableHoles(Vec<HoleFault>),
    /// The rack cannot hold the minimal feasible tool set under `FixedToolset`.
    RackTooSmall { minimal: usize, capacity: usize },
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

/// A length quantised to whole micrometres — the domain precision for all
/// diameter/fit comparisons and tie-breaks.
fn micron(length: Length) -> i64 {
    length.as_um().round() as i64
}

/// The axial length from a twist-drill tip to its full-diameter shoulder: the point
/// is a cone of included angle `point_angle`, so `Lp = (D/2) / tan(angle/2)`.
/// Flat/blunt tools (≥180°, e.g. routers/end-mills) have no point.
fn point_length_mm(diameter_mm: f64, point_angle_deg: f64) -> f64 {
    if point_angle_deg >= 180.0 {
        return 0.0;
    }
    let half = (point_angle_deg / 2.0).to_radians();
    let tangent = half.tan();
    if tangent <= 1e-9 {
        return 0.0;
    }
    (diameter_mm / 2.0) / tangent
}

/// The Z-feasibility verdict for one (tool, hole) pair (§2½).
struct ZFit {
    feasible: bool,
    /// Usable length reaches the required plunge (or flute length unknown).
    reach_ok: bool,
    /// Breakthrough stays within the fixture's below-board clearance.
    bed_ok: bool,
    /// The plunge target `T + Lp + m`.
    z_bottom_mm: f64,
    /// True when the tool has no declared flute length (reach not verified).
    reach_unverified: bool,
}

/// Can this tool make the hole cleanly through the board without the tip reaching
/// the machine bed? Requires both **reach** (usable length ≥ plunge) and **bed
/// safety** (breakthrough ≤ fixture clearance). An absent flute length is not a
/// disqualifier here — the caller raises a "reach not verified" warning instead.
fn z_feasibility(diameter_mm: f64, point_angle_deg: f64, flute_mm: Option<f64>, setup: &Setup) -> ZFit {
    let thickness = setup.board_thickness.as_mm();
    let clearance = setup.bed_clearance.as_mm();
    let margin = setup.breakthrough_margin.as_mm();

    let point = point_length_mm(diameter_mm, point_angle_deg);
    let breakthrough = point + margin; // distance below the board's bottom face
    let z_break = thickness + breakthrough; // plunge past the top surface

    let bed_ok = breakthrough <= clearance + EPS_MM;
    let (reach_ok, reach_unverified) = match flute_mm {
        Some(flute) => (flute + EPS_MM >= z_break, false),
        None => (true, true),
    };

    ZFit { feasible: bed_ok && reach_ok, reach_ok, bed_ok, z_bottom_mm: z_break, reach_unverified }
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

/// One feasible (tool, hole) pairing, with its precomputed score.
struct Candidate<'a> {
    tool: &'a Tool,
    /// Stable position in the input tool slice (final deterministic tie-break).
    index: usize,
    strategy: Strategy,
    tool_um: i64,
    fit_um: i64,
    z_bottom_mm: f64,
    reach_unverified: bool,
    score: f64,
}

/// The working state for one hole: its sorted candidates (best first) and the
/// currently chosen one.
struct HoleWork<'a> {
    demand: &'a HoleDemand,
    match_um: i64,
    candidates: Vec<Candidate<'a>>,
    chosen: usize,
    changed_by_shrink: bool,
}

impl<'a> HoleWork<'a> {
    fn chosen_tool_id(&self) -> &str {
        self.candidates[self.chosen].tool.id.as_str()
    }
}

/// The pref term: preferred favoured, not-preferred penalised.
fn preference_score(preference: ToolPreference) -> f64 {
    match preference {
        ToolPreference::Preferred => 1.0,
        ToolPreference::Neutral => 0.0,
        ToolPreference::NotPreferred => -1.0,
    }
}

/// `S = Ws·strategy − Wf·fit + Wp·pref + Wr·reuse` (higher is better). `reuse` is
/// membership in the already-in-rack set (mandatory ∪ fixed), nudging the picker
/// toward tools that cost no extra change.
fn score_candidate(
    tool: &Tool,
    strategy: Strategy,
    fit_um: i64,
    match_um: i64,
    weights: &Weights,
    reuse_set: &BTreeSet<String>,
) -> f64 {
    let strategy_term = if strategy == Strategy::Drill { 1.0 } else { 0.0 };
    let normalised_fit = fit_um as f64 / match_um.max(1) as f64;
    let reuse = if reuse_set.contains(&tool.id) { 1.0 } else { 0.0 };
    weights.strategy * strategy_term - weights.fit * normalised_fit
        + weights.pref * preference_score(tool.preference)
        + weights.reuse * reuse
}

/// Deterministic ordering of candidates: higher score, then smaller diameter, then
/// earlier input position.
fn better_first(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    if (a.score - b.score).abs() > SCORE_EPS {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        a.tool_um.cmp(&b.tool_um).then(a.index.cmp(&b.index))
    }
}

/// Whether a tool is a drill / a router-capable cutter.
fn is_drill(tool: &Tool) -> bool {
    matches!(ToolKind::from_kind_label(&tool.kind), ToolKind::Drillbit)
}
fn is_router(tool: &Tool) -> bool {
    matches!(ToolKind::from_kind_label(&tool.kind), ToolKind::Routerbit | ToolKind::Endmill)
}

/// Builds the candidate list for one hole. Returns the candidates plus a flag: a
/// size-matching drill existed but every one failed Z-feasibility (so an empty list
/// means *depth-infeasible* rather than *no size match*).
fn build_candidates<'a>(
    hole: &HoleDemand,
    tools: &'a [Tool],
    cfg: &AssignConfig,
    setup: &Setup,
    reuse_set: &BTreeSet<String>,
) -> (Vec<Candidate<'a>>, bool) {
    let match_mm = hole.match_length().as_mm();
    let match_um = micron(hole.match_length());
    let lower = match_mm - cfg.undersize.effective_mm(match_mm);
    let upper = match_mm + cfg.oversize.effective_mm(match_mm);

    let mut candidates = Vec::new();
    let mut size_matched_but_infeasible = false;

    for (index, tool) in tools.iter().enumerate() {
        if tool.status != ToolStatus::InStock {
            continue;
        }
        let diameter_mm = tool.diameter.as_mm();
        let angle = tool.point_angle.as_degrees();

        if is_drill(tool) {
            // A drill matches when it lands in the allowance window.
            if diameter_mm >= lower - EPS_MM && diameter_mm <= upper + EPS_MM {
                let z = z_feasibility(diameter_mm, angle, tool.flute_length.map(Length::as_mm), setup);
                if z.feasible {
                    let tool_um = micron(tool.diameter);
                    let fit_um = (tool_um - match_um).abs();
                    let score = score_candidate(tool, Strategy::Drill, fit_um, match_um, &cfg.weights, reuse_set);
                    candidates.push(Candidate {
                        tool,
                        index,
                        strategy: Strategy::Drill,
                        tool_um,
                        fit_um,
                        z_bottom_mm: z.z_bottom_mm,
                        reach_unverified: z.reach_unverified,
                        score,
                    });
                } else {
                    size_matched_but_infeasible = true;
                }
            }
        }

        // Router fallback: any router strictly smaller than the hole can helical-mill
        // it — considered for every routable hole when enabled, but never preferred.
        if cfg.allow_routing_holes && hole.routable && is_router(tool) && diameter_mm < match_mm - EPS_MM {
            let z = z_feasibility(diameter_mm, angle, tool.flute_length.map(Length::as_mm), setup);
            if z.feasible {
                let tool_um = micron(tool.diameter);
                // A router interpolates to the exact size, so fit is not a size error.
                let score = score_candidate(tool, Strategy::Route, 0, match_um, &cfg.weights, reuse_set);
                candidates.push(Candidate {
                    tool,
                    index,
                    strategy: Strategy::Route,
                    tool_um,
                    fit_um: 0,
                    z_bottom_mm: z.z_bottom_mm,
                    reach_unverified: z.reach_unverified,
                    score,
                });
            }
        }
    }

    candidates.sort_by(better_first);
    (candidates, size_matched_but_infeasible)
}

/// The closest in-stock drills to `target`, formatted for a fault diagnostic.
fn nearest_drills(tools: &[Tool], target: Length) -> Vec<String> {
    let target_um = micron(target);
    let mut drills: Vec<&Tool> = tools
        .iter()
        .filter(|t| t.status == ToolStatus::InStock && is_drill(t))
        .collect();
    drills.sort_by_key(|t| (micron(t.diameter) - target_um).abs());
    drills
        .into_iter()
        .take(3)
        .map(|t| format!("{} ({:.3} mm)", t.display_name(), t.diameter.as_mm()))
        .collect()
}

// ---------------------------------------------------------------------------
// Assignment
// ---------------------------------------------------------------------------

/// Assigns a tool to every hole and lays out the rack for one machining step.
///
/// Returns the per-hole assignment + rack + shrink record + diagnostics, or a hard
/// error when a hole is uncoverable or the rack is too small under `FixedToolset`.
pub fn assign(
    holes: &[HoleDemand],
    tools: &[Tool],
    cfg: &AssignConfig,
    rack: &RackSpec,
    setup: &Setup,
) -> Result<ToolAssignment, AssignError> {
    let fixed_ids: BTreeSet<String> = rack.fixed.iter().map(|(_, id)| id.clone()).collect();
    let mandatory_ids: BTreeSet<String> = rack.mandatory.iter().cloned().collect();
    // Tools already in the rack cost no extra change, so the picker mildly prefers them.
    let reuse_set: BTreeSet<String> = fixed_ids.union(&mandatory_ids).cloned().collect();

    // 1. Build candidates; collect uncoverable holes as hard faults.
    let mut works: Vec<HoleWork> = Vec::with_capacity(holes.len());
    let mut faults: Vec<HoleFault> = Vec::new();
    for hole in holes {
        let (candidates, size_matched_but_infeasible) =
            build_candidates(hole, tools, cfg, setup, &reuse_set);
        if candidates.is_empty() {
            faults.push(HoleFault {
                hole_id: hole.id.clone(),
                kind: hole.kind,
                target_um: micron(hole.match_length()),
                reason: if size_matched_but_infeasible {
                    FaultReason::DepthInfeasible
                } else {
                    FaultReason::NoSizeMatch
                },
                nearest: nearest_drills(tools, hole.match_length()),
            });
            continue;
        }
        works.push(HoleWork {
            demand: hole,
            match_um: micron(hole.match_length()),
            candidates,
            chosen: 0, // sorted best-first
            changed_by_shrink: false,
        });
    }
    if !faults.is_empty() {
        return Err(AssignError::UncoverableHoles(faults));
    }

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut removed: Vec<Removal> = Vec::new();

    // 2. Shrink the working set to the rack capacity by minimum-regret removal.
    shrink_to_capacity(&mut works, tools, rack, &fixed_ids, &mandatory_ids, &mut removed);

    // 3. Capacity outcome.
    let rack_tools = current_rack_tools(&works, &fixed_ids, &mandatory_ids);
    if rack_tools.len() > rack.capacity {
        match rack.policy {
            OverflowPolicy::FixedToolset => {
                return Err(AssignError::RackTooSmall {
                    minimal: rack_tools.len(),
                    capacity: rack.capacity,
                });
            }
            OverflowPolicy::AllowReload | OverflowPolicy::AllowHybrid => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "{} tools exceed the {}-slot rack; operator reload/manual tool changes required",
                        rack_tools.len(),
                        rack.capacity
                    ),
                });
            }
        }
    }

    // 4. Pilot pass for routed holes (reusing drills already in the rack).
    let pilots = assign_pilots(&works, tools, cfg, setup, &rack_tools, &mut diagnostics);

    // 5. Reach-not-verified warning for chosen tools with no flute length.
    warn_unverified_reach(&works, &mut diagnostics);

    // 6. Materialise the outputs.
    let assignment_holes: Vec<HoleAssignment> = works
        .iter()
        .enumerate()
        .map(|(hole_index, work)| {
            let candidate = &work.candidates[work.chosen];
            HoleAssignment {
                hole_id: work.demand.id.clone(),
                tool_id: candidate.tool.id.clone(),
                strategy: candidate.strategy,
                pilot_tool_id: pilots[hole_index].clone(),
                fit_error_um: candidate.fit_um,
                z_bottom: Length::from_mm(candidate.z_bottom_mm),
                changed_by_shrink: work.changed_by_shrink,
            }
        })
        .collect();

    let rack_layout = build_rack_layout(&rack_tools, tools, rack);

    Ok(ToolAssignment { holes: assignment_holes, rack: rack_layout, removed, diagnostics })
}

/// The set of tools currently occupying the rack: those chosen by a hole, plus the
/// mandatory (routing) and fixed tools that are always present.
fn current_rack_tools(
    works: &[HoleWork],
    fixed_ids: &BTreeSet<String>,
    mandatory_ids: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = works.iter().map(|w| w.chosen_tool_id().to_string()).collect();
    set.extend(mandatory_ids.iter().cloned());
    set.extend(fixed_ids.iter().cloned());
    set
}

/// The best candidate for a hole whose tool is in `allowed`, or `None` if the hole
/// has no candidate among the permitted tools. Candidates are pre-sorted best-first,
/// so the first match is the best available.
fn best_in_set(work: &HoleWork, allowed: &BTreeSet<String>) -> Option<usize> {
    work.candidates.iter().position(|c| allowed.contains(&c.tool.id))
}

/// Iteratively removes the non-mandatory tool of minimum global regret until the
/// rack fits or no removal preserves feasibility (§8.7 step 5).
fn shrink_to_capacity(
    works: &mut [HoleWork],
    tools: &[Tool],
    rack: &RackSpec,
    fixed_ids: &BTreeSet<String>,
    mandatory_ids: &BTreeSet<String>,
    removed: &mut Vec<Removal>,
) {
    // Diameter lookup for deterministic removal tie-breaks.
    let tool_um = |id: &str| -> i64 {
        tools.iter().find(|t| t.id == id).map(|t| micron(t.diameter)).unwrap_or(0)
    };

    loop {
        let rack_tools = current_rack_tools(works, fixed_ids, mandatory_ids);
        if rack_tools.len() <= rack.capacity {
            return;
        }

        // Removable = tools a hole currently uses that are neither fixed nor mandatory.
        let in_use: BTreeSet<String> = works.iter().map(|w| w.chosen_tool_id().to_string()).collect();
        let removable: Vec<String> = in_use
            .iter()
            .filter(|id| !fixed_ids.contains(*id) && !mandatory_ids.contains(*id))
            .cloned()
            .collect();
        if removable.is_empty() {
            return; // nothing left to remove — overflow handled by the caller
        }

        // Pick the removable tool with the least regret (loss from reassigning its
        // holes to their next-best tool already in the rack; infeasible ⇒ skip).
        let mut best: Option<(f64, i64, String)> = None;
        for candidate_id in &removable {
            let allowed: BTreeSet<String> =
                rack_tools.iter().filter(|id| *id != candidate_id).cloned().collect();
            let mut regret = 0.0;
            let mut feasible = true;
            for work in works.iter() {
                if work.chosen_tool_id() != candidate_id {
                    continue;
                }
                match best_in_set(work, &allowed) {
                    Some(new_idx) => {
                        let old = &work.candidates[work.chosen];
                        let new = &work.candidates[new_idx];
                        regret += old.score - new.score;
                        if old.strategy == Strategy::Drill && new.strategy == Strategy::Route {
                            regret += DRILL_TO_ROUTE_PENALTY;
                        }
                    }
                    None => {
                        feasible = false;
                        break;
                    }
                }
            }
            if !feasible {
                continue;
            }
            let key = (regret, tool_um(candidate_id), candidate_id.clone());
            if best.as_ref().map(|b| key < *b).unwrap_or(true) {
                best = Some(key);
            }
        }

        let Some((regret, _, tool_id)) = best else {
            return; // no feasible removal — overflow handled by the caller
        };

        // Apply: reassign the removed tool's holes to their best remaining option.
        let allowed: BTreeSet<String> =
            rack_tools.iter().filter(|id| **id != tool_id).cloned().collect();
        for work in works.iter_mut() {
            if work.chosen_tool_id() == tool_id {
                if let Some(new_idx) = best_in_set(work, &allowed) {
                    work.chosen = new_idx;
                    work.changed_by_shrink = true;
                }
            }
        }
        removed.push(Removal {
            tool_id,
            reason: "removed to fit rack capacity (minimum regret)".to_string(),
            regret,
        });
    }
}

/// For each routed hole, selects a pilot drill already in the rack: the largest
/// in-rack drill strictly larger than the router yet no larger than the hole, and
/// itself Z-feasible. Warns when pilots were requested but none is available.
fn assign_pilots(
    works: &[HoleWork],
    tools: &[Tool],
    cfg: &AssignConfig,
    setup: &Setup,
    rack_tools: &BTreeSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Option<String>> {
    let mut pilots = vec![None; works.len()];
    if !cfg.pilot {
        return pilots;
    }

    let mut routed = 0usize;
    let mut without_pilot = 0usize;
    for (index, work) in works.iter().enumerate() {
        let chosen = &work.candidates[work.chosen];
        if chosen.strategy != Strategy::Route {
            continue;
        }
        routed += 1;
        let router_um = chosen.tool_um;
        let hole_um = work.match_um;

        let pilot = tools
            .iter()
            .filter(|t| {
                rack_tools.contains(&t.id)
                    && is_drill(t)
                    && t.status == ToolStatus::InStock
                    && micron(t.diameter) > router_um
                    && micron(t.diameter) <= hole_um
                    && z_feasibility(
                        t.diameter.as_mm(),
                        t.point_angle.as_degrees(),
                        t.flute_length.map(Length::as_mm),
                        setup,
                    )
                    .feasible
            })
            .max_by_key(|t| micron(t.diameter));

        match pilot {
            Some(tool) => pilots[index] = Some(tool.id.clone()),
            None => without_pilot += 1,
        }
    }

    if routed > 0 && without_pilot > 0 {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: format!(
                "pilot holes unavailable for {without_pilot} of {routed} routed hole(s); no suitable drill in the rack — machined as full-route"
            ),
        });
    }
    pilots
}

/// Warns once, listing chosen tools whose reach could not be verified (no declared
/// flute length). The bed-safety check still held; only reach is unverified.
fn warn_unverified_reach(works: &[HoleWork], diagnostics: &mut Vec<Diagnostic>) {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for work in works {
        let chosen = &work.candidates[work.chosen];
        if chosen.reach_unverified {
            names.insert(chosen.tool.display_name());
        }
    }
    if !names.is_empty() {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            message: format!(
                "reach not verified (no flute length) for: {}",
                names.into_iter().collect::<Vec<_>>().join(", ")
            ),
        });
    }
}

/// Lays out the rack as **one row per distinct tool** the job needs, on the
/// toolset's real slots. A fixed tool keeps its pinned slot (its first, if pinned in
/// several); every other tool fills the next `spare_slot` in order, by diameter then
/// id for determinism. Slots absent from both `fixed` and `spare_slots` (e.g.
/// do-not-use) are never used, so they are simply skipped. Because `rack_tools` is a
/// set, no tool is ever listed twice.
fn build_rack_layout(rack_tools: &BTreeSet<String>, tools: &[Tool], rack: &RackSpec) -> Vec<SlotAssignment> {
    // The slot each fixed tool occupies (its first, when pinned in several).
    let mut fixed_slot: BTreeMap<&str, u8> = BTreeMap::new();
    for (slot, id) in &rack.fixed {
        fixed_slot.entry(id.as_str()).or_insert(*slot);
    }
    let fixed_slots: BTreeSet<u8> = rack.fixed.iter().map(|(slot, _)| *slot).collect();

    let tool_um = |id: &str| -> i64 {
        tools.iter().find(|t| t.id == id).map(|t| micron(t.diameter)).unwrap_or(0)
    };

    let mut layout: Vec<SlotAssignment> = Vec::new();
    let mut auto: Vec<&String> = Vec::new();
    for id in rack_tools {
        if let Some(&slot) = fixed_slot.get(id.as_str()) {
            layout.push(SlotAssignment { slot, tool_id: id.clone() });
        } else {
            auto.push(id);
        }
    }

    auto.sort_by(|a, b| tool_um(a).cmp(&tool_um(b)).then(a.as_str().cmp(b.as_str())));
    // Auto tools take the toolset's spare slots in index order (never a fixed slot).
    let mut spare = rack.spare_slots.iter().copied().filter(|slot| !fixed_slots.contains(slot));
    for id in auto {
        if let Some(slot) = spare.next() {
            layout.push(SlotAssignment { slot, tool_id: id.clone() });
        }
    }
    layout.sort_by_key(|s| s.slot);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use units::Angle;

    // --- fixtures ---------------------------------------------------------

    /// A stock tool with just the fields the assigner reads. `flute_mm = None`
    /// leaves the reach check unverified.
    fn tool(id: &str, kind: &str, diameter_mm: f64, flute_mm: Option<f64>, preference: ToolPreference) -> Tool {
        Tool {
            id: id.to_string(),
            composite_name: format!("{kind} {diameter_mm}mm"),
            name: format!("{kind} {diameter_mm}mm"),
            kind: kind.to_string(),
            diameter: Length::from_mm(diameter_mm),
            catalog_diameter: None,
            point_angle: Angle::from_degrees(if kind == "Drill" { 118.0 } else { 180.0 }),
            catalog_point_angle: None,
            flute_length: flute_mm.map(Length::from_mm),
            feed_rate: None,
            catalog_feed_rate: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: ToolStatus::InStock,
            preference,
            source_catalog: "Test".to_string(),
            manufacturer: None,
            sku: None,
        }
    }

    fn drill(id: &str, diameter_mm: f64) -> Tool {
        tool(id, "Drill", diameter_mm, Some(30.0), ToolPreference::Neutral)
    }
    fn router(id: &str, diameter_mm: f64) -> Tool {
        tool(id, "Router", diameter_mm, Some(30.0), ToolPreference::Neutral)
    }

    fn round_hole(id: &str, diameter_mm: f64) -> HoleDemand {
        HoleDemand {
            id: id.to_string(),
            kind: DemandKind::Pth,
            target: Length::from_mm(diameter_mm),
            minor_axis: None,
            plated: true,
            routable: true,
        }
    }

    /// A generous, roomy setup: a thin board with ample bed clearance and long bits,
    /// so Z-feasibility never gets in the way unless a test makes it.
    fn roomy_setup() -> Setup {
        Setup {
            board_thickness: Length::from_mm(1.6),
            bed_clearance: Length::from_mm(5.0),
            breakthrough_margin: Length::from_mm(0.5),
        }
    }

    fn config(allow_routing: bool) -> AssignConfig {
        AssignConfig {
            allow_routing_holes: allow_routing,
            drill_first: true,
            pilot: false,
            oversize: Allowance { relative: 0.08, max: Length::from_mm(0.10) },
            undersize: Allowance { relative: 0.06, max: Length::from_mm(0.08) },
            weights: Weights::default(),
        }
    }

    fn rack(capacity: usize) -> RackSpec {
        RackSpec {
            capacity,
            fixed: vec![],
            spare_slots: (1..=capacity as u8).collect(),
            mandatory: vec![],
            policy: OverflowPolicy::FixedToolset,
        }
    }

    // --- tests ------------------------------------------------------------

    #[test]
    fn picks_the_closest_feasible_drill() {
        let tools = vec![drill("a", 1.0), drill("b", 1.05)];
        let holes = vec![round_hole("h1", 1.0)];
        let out = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].tool_id, "a");
        assert_eq!(out.holes[0].strategy, Strategy::Drill);
        assert_eq!(out.holes[0].fit_error_um, 0);
    }

    #[test]
    fn tie_break_smaller_diameter_wins_when_scores_equal() {
        // Both drills are 1 µm from the 1.0 mm target — equal fit, so the smaller wins.
        let tools = vec![drill("over", 1.001), drill("under", 0.999)];
        let holes = vec![round_hole("h1", 1.0)];
        let out = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].tool_id, "under");
    }

    #[test]
    fn sub_micron_diameters_are_treated_as_equal() {
        // 0.4 µm apart → same quantised diameter → tie broken by input order (first wins).
        let tools = vec![drill("first", 1.0000), drill("second", 1.0004)];
        let holes = vec![round_hole("h1", 1.0)];
        let out = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].tool_id, "first");
    }

    #[test]
    fn empty_candidate_set_is_an_immediate_error() {
        // A 5 mm hole, only a 1 mm drill (out of window), routing off → uncoverable.
        let tools = vec![drill("a", 1.0)];
        let holes = vec![round_hole("h1", 5.0)];
        let err = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap_err();
        match err {
            AssignError::UncoverableHoles(faults) => {
                assert_eq!(faults.len(), 1);
                assert_eq!(faults[0].reason, FaultReason::NoSizeMatch);
                assert!(!faults[0].nearest.is_empty(), "diagnostics list nearest stock");
            }
            other => panic!("expected UncoverableHoles, got {other:?}"),
        }
    }

    #[test]
    fn z_feasibility_rejects_a_bit_too_short_to_reach_through() {
        // Board 1.6 mm; a 1 mm 118° drill needs ~1.6 + 0.30 + 0.5 ≈ 2.4 mm of reach,
        // but this bit only has 1.0 mm of flute → depth-infeasible, no router → error.
        let tools = vec![drill("short", 1.0)];
        let short_flute = Tool { flute_length: Some(Length::from_mm(1.0)), ..tools[0].clone() };
        let holes = vec![round_hole("h1", 1.0)];
        let err = assign(&holes, &[short_flute], &config(false), &rack(4), &roomy_setup()).unwrap_err();
        match err {
            AssignError::UncoverableHoles(faults) => assert_eq!(faults[0].reason, FaultReason::DepthInfeasible),
            other => panic!("expected DepthInfeasible, got {other:?}"),
        }
    }

    #[test]
    fn z_feasibility_rejects_breakthrough_into_the_bed() {
        // Ample reach, but only 0.1 mm of clearance under the board while the point +
        // margin need ~0.8 mm → the tip would hit the bed → depth-infeasible.
        let tools = vec![drill("a", 1.0)];
        let tight_bed = Setup { bed_clearance: Length::from_mm(0.1), ..roomy_setup() };
        let holes = vec![round_hole("h1", 1.0)];
        let err = assign(&holes, &tools, &config(false), &rack(4), &tight_bed).unwrap_err();
        assert!(matches!(err, AssignError::UncoverableHoles(_)));
    }

    #[test]
    fn z_bottom_is_the_computed_plunge_depth() {
        let tools = vec![drill("a", 1.0)];
        let holes = vec![round_hole("h1", 1.0)];
        let out = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap();
        // T=1.6, m=0.5, Lp = 0.5 / tan(59°) ≈ 0.300 → z_bottom ≈ 2.40 mm.
        let expected = 1.6 + 0.5 + point_length_mm(1.0, 118.0);
        assert!((out.holes[0].z_bottom.as_mm() - expected).abs() < 1e-3);
    }

    #[test]
    fn routes_a_hole_with_no_feasible_drill_when_fallback_is_on() {
        // 3 mm hole, no drill near it, a 1 mm router present → routed (not drilled).
        let tools = vec![drill("d", 1.0), router("r", 1.0)];
        let holes = vec![round_hole("h1", 3.0)];
        let out = assign(&holes, &tools, &config(true), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].strategy, Strategy::Route);
        assert_eq!(out.holes[0].tool_id, "r");
    }

    #[test]
    fn no_router_and_no_drill_errors_when_fallback_is_off() {
        let tools = vec![router("r", 1.0)];
        let holes = vec![round_hole("h1", 3.0)];
        let err = assign(&holes, &tools, &config(false), &rack(4), &roomy_setup()).unwrap_err();
        assert!(matches!(err, AssignError::UncoverableHoles(_)));
    }

    #[test]
    fn drilling_is_preferred_over_routing_when_both_feasible() {
        // Both a matching drill and a small router can do a 2 mm hole → drill wins.
        let tools = vec![router("r", 1.0), drill("d", 2.0)];
        let holes = vec![round_hole("h1", 2.0)];
        let out = assign(&holes, &tools, &config(true), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].strategy, Strategy::Drill);
        assert_eq!(out.holes[0].tool_id, "d");
    }

    #[test]
    fn large_hole_whose_only_drill_is_depth_infeasible_falls_to_routing() {
        // A right-size 3 mm drill exists but is too short; a small router can do it.
        let big_drill = tool("big", "Drill", 3.0, Some(1.0), ToolPreference::Neutral); // too short
        let small_router = router("r", 1.0);
        let holes = vec![round_hole("h1", 3.0)];
        let out = assign(&holes, &[big_drill, small_router], &config(true), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].strategy, Strategy::Route);
        assert_eq!(out.holes[0].tool_id, "r");
    }

    #[test]
    fn shrink_removes_the_minimum_regret_tool_to_fit_capacity() {
        // Three holes want three drills, but the two close 1.0/1.05 mm holes can share
        // one bit (each is within the other's allowance). Capacity 2 → one of the two
        // small drills is dropped and both small holes collapse onto the survivor. (Which
        // of the two is removed depends on the normalized-fit regret; the 2 mm drill,
        // the only one that can do the 2 mm hole, must survive.)
        let tools = vec![drill("d1", 1.0), drill("d105", 1.05), drill("d2", 2.0)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h105", 1.05), round_hole("h2", 2.0)];
        let out = assign(&holes, &tools, &config(false), &rack(2), &roomy_setup()).unwrap();

        assert_eq!(out.rack.len(), 2, "fits capacity");
        assert_eq!(out.removed.len(), 1);
        assert_ne!(out.removed[0].tool_id, "d2", "the only 2 mm-capable drill must survive");

        let h1 = out.holes.iter().find(|h| h.hole_id == "h1").unwrap();
        let h105 = out.holes.iter().find(|h| h.hole_id == "h105").unwrap();
        assert_eq!(h1.tool_id, h105.tool_id, "both small holes share one surviving drill");
        assert!(h1.changed_by_shrink ^ h105.changed_by_shrink, "exactly one hole was reassigned");
        // The 2 mm hole is untouched on its own drill.
        let h2 = out.holes.iter().find(|h| h.hole_id == "h2").unwrap();
        assert_eq!(h2.tool_id, "d2");
        assert!(!h2.changed_by_shrink);
    }

    #[test]
    fn mandatory_routing_tools_are_never_removed_by_shrink() {
        // Two drills + a mandatory router, capacity 2 → a drill is dropped, never the router.
        let tools = vec![drill("d1", 1.0), drill("d103", 1.03), router("R", 0.5)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h103", 1.03)];
        let mut spec = rack(2);
        spec.mandatory = vec!["R".to_string()];
        let out = assign(&holes, &tools, &config(true), &spec, &roomy_setup()).unwrap();

        assert!(out.removed.iter().all(|r| r.tool_id != "R"), "mandatory router must survive");
        assert!(out.rack.iter().any(|s| s.tool_id == "R"), "mandatory router stays in the rack");
        assert!(out.rack.len() <= 2);
    }

    #[test]
    fn rack_too_small_is_a_hard_error_under_fixed_toolset() {
        // Two holes needing two distinct, non-substitutable drills; capacity 1.
        let tools = vec![drill("d1", 1.0), drill("d5", 5.0)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h5", 5.0)];
        let err = assign(&holes, &tools, &config(false), &rack(1), &roomy_setup()).unwrap_err();
        match err {
            AssignError::RackTooSmall { minimal, capacity } => {
                assert_eq!(minimal, 2);
                assert_eq!(capacity, 1);
            }
            other => panic!("expected RackTooSmall, got {other:?}"),
        }
    }

    #[test]
    fn overflow_under_allow_reload_warns_instead_of_failing() {
        let tools = vec![drill("d1", 1.0), drill("d5", 5.0)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h5", 5.0)];
        let mut spec = rack(1);
        spec.policy = OverflowPolicy::AllowReload;
        let out = assign(&holes, &tools, &config(false), &spec, &roomy_setup()).unwrap();
        assert!(out.diagnostics.iter().any(|d| d.severity == Severity::Warning));
        assert_eq!(out.holes.len(), 2, "both holes still assigned");
    }

    #[test]
    fn a_tool_pinned_in_several_slots_is_listed_once() {
        // A toolset that redundantly fixes the same drill in three slots must show it
        // once — not three rows — and the rack row count is the distinct-tool count.
        let tools = vec![drill("dup", 1.0), drill("other", 2.0)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h2", 2.0)];
        let mut spec = rack(6);
        spec.fixed = vec![(1, "dup".into()), (2, "dup".into()), (3, "dup".into())];
        let out = assign(&holes, &tools, &config(false), &spec, &roomy_setup()).unwrap();
        assert_eq!(out.rack.iter().filter(|s| s.tool_id == "dup").count(), 1, "no duplicate slots");
        assert_eq!(out.rack.len(), 2, "one row per distinct tool");
    }

    #[test]
    fn auto_tools_fill_spare_slots_and_skip_excluded_ones() {
        // spare_slots omits slot 3 (a do-not-use slot upstream); tools must skip it.
        let tools = vec![drill("a", 1.0), drill("b", 2.0), drill("c", 3.0)];
        let holes = vec![round_hole("h1", 1.0), round_hole("h2", 2.0), round_hole("h3", 3.0)];
        let mut spec = rack(3);
        spec.spare_slots = vec![1, 2, 4]; // slot 3 excluded (do-not-use)
        let out = assign(&holes, &tools, &config(false), &spec, &roomy_setup()).unwrap();
        let slots: Vec<u8> = out.rack.iter().map(|s| s.slot).collect();
        assert_eq!(slots, vec![1, 2, 4], "fills spare slots in order, skipping the excluded slot");
        assert!(!out.rack.iter().any(|s| s.slot == 3), "the excluded slot is never used");
    }

    #[test]
    fn pilot_drill_is_selected_for_a_routed_hole_when_available() {
        // 3 mm hole routed by a 1 mm router; a 2 mm drill in the rack pilots it.
        let tools = vec![router("r", 1.0), drill("pilot", 2.0)];
        let holes = vec![round_hole("h1", 3.0)];
        let mut cfg = config(true);
        cfg.pilot = true;
        // Force both tools into the rack via mandatory so the pilot drill is present.
        let mut spec = rack(4);
        spec.mandatory = vec!["pilot".to_string()];
        let out = assign(&holes, &tools, &cfg, &spec, &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].strategy, Strategy::Route);
        assert_eq!(out.holes[0].pilot_tool_id.as_deref(), Some("pilot"));
    }

    #[test]
    fn out_of_stock_tools_are_not_candidates() {
        let mut d = drill("a", 1.0);
        d.status = ToolStatus::OutOfStock;
        let holes = vec![round_hole("h1", 1.0)];
        let err = assign(&holes, &[d], &config(false), &rack(4), &roomy_setup()).unwrap_err();
        assert!(matches!(err, AssignError::UncoverableHoles(_)));
    }

    #[test]
    fn preferred_tool_wins_over_a_neutral_equal_fit() {
        // Two drills equidistant (±1 µm): the preferred one wins over smaller-diameter.
        let over = tool("over", "Drill", 1.001, Some(30.0), ToolPreference::Preferred);
        let under = tool("under", "Drill", 0.999, Some(30.0), ToolPreference::Neutral);
        let holes = vec![round_hole("h1", 1.0)];
        let out = assign(&holes, &[over, under], &config(false), &rack(4), &roomy_setup()).unwrap();
        assert_eq!(out.holes[0].tool_id, "over");
    }
}
