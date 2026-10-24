//! What the setup sheet says, as plain data already formatted for the operator.
//!
//! Built by [`build`] from [`Sources`]: explicit inputs rather than the live context, so the
//! wording and the arithmetic can be tested without a datastore or a KiCad. Everything in it
//! is read from the same places the app's own views read it — the board snapshot, the
//! tooling plan's racks, the machining plan's tool blocks, the generated programs — so the
//! sheet cannot describe a job the app is not about to run.

use std::collections::BTreeMap;

use pcb::{BoardSnapshot, HoleKind, StitchResult};
use units::user_format::{format_feed_display, format_length_display, format_rotational_speed_display};
use units::{Length, UserUnitSystem};

use crate::data::model::operations::operation_label;
use crate::data::model::profiles::{FixtureProfile, MachineProfile, ToolsetProfile};
use crate::data::model::Tool;
use crate::gcode::feeds::{self, MachineLimits};
use crate::gcode::plan::{MachiningPlan, StepPlan};
use crate::gcode::program::ToolFeed;
use crate::runtime::StepProgram;
use crate::runtime::tooling::{RackStepView, SlotChange, StepOutcome, StepRaw, ToolingPlan};

/// What one engraving step's isolation pass came to, read off the held contours.
///
/// Owned and flat so [`build`] needs no access to the isolation worker; collected by
/// `super::collect`.
#[derive(Clone, Debug, PartialEq)]
pub struct EngraveFacts {
    pub back: bool,
    pub tool: String,
    pub requested: Length,
    pub achieved: Length,
    /// `None` while the contours are still being worked out.
    pub held: Option<HeldFacts>,
}

/// The counts and lengths of a finished isolation pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeldFacts {
    pub contours: usize,
    pub length_nm: f64,
    pub narrowed: usize,
    pub uncut_nm: f64,
    pub islands_removed: usize,
    pub milled_pieces: usize,
    pub copper_layers: u32,
}

/// Everything [`build`] reads, gathered from the live context by `super::collect`.
pub struct Sources<'a> {
    pub board: Option<&'a BoardSnapshot>,
    pub stitched: Option<&'a StitchResult>,
    pub profile_name: String,
    pub generated: String,
    pub version: String,
    pub unit: UserUnitSystem,
    pub steps: &'a [StepRaw],
    pub tooling: &'a ToolingPlan,
    pub plan: &'a MachiningPlan,
    pub programs: &'a [StepProgram],
    /// The file each step's program is being written as, by step index. A step missing here
    /// is one the operator left out of the export.
    pub program_files: &'a BTreeMap<usize, String>,
    /// One per step, `Some` for the steps that engrave.
    pub engraving: Vec<Option<EngraveFacts>>,
    /// One per step: the clamps its machine puts on every feed.
    pub limits: Vec<MachineLimits>,
    pub machines: &'a [MachineProfile],
    pub fixtures: &'a [FixtureProfile],
    pub toolsets: &'a [ToolsetProfile],
    pub tools: &'a [Tool],
    pub tool_feeds: &'a BTreeMap<String, ToolFeed>,
}

/// The whole sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupSheet {
    pub board_name: String,
    pub profile_name: String,
    pub generated: String,
    pub version: String,
    /// Size, thickness, holes, outline: one line each.
    pub board: Vec<(String, String)>,
    /// Kind, size, count — one row per distinct drill or slot.
    pub holes: Vec<Vec<String>>,
    pub engraving: Vec<EngravingSummary>,
    pub steps: Vec<StepSheet>,
}

/// One engraving step's isolation pass, for the board section.
#[derive(Clone, Debug, PartialEq)]
pub struct EngravingSummary {
    pub title: String,
    pub facts: Vec<(String, String)>,
}

/// One step's page.
#[derive(Clone, Debug, PartialEq)]
pub struct StepSheet {
    pub number: usize,
    pub name: String,
    pub back: bool,
    /// `Front` or `Back`.
    pub face: String,
    /// The machine, and whether it changes tools itself.
    pub machine: String,
    /// The kinds of work, e.g. `Isolation, Drilling`.
    pub work: String,
    pub operations: Vec<String>,
    pub setup: Vec<(String, String)>,
    pub rack: RackSection,
    pub tools: Vec<ToolRow>,
    pub notes: Vec<String>,
    /// The program file, or why there is none.
    pub program: String,
    /// The bare file name, for the summary table; empty when the step has no exported file.
    pub program_file: String,
}

/// How the tools get into the spindle for one step.
#[derive(Clone, Debug, PartialEq)]
pub enum RackSection {
    /// A machine with a tool changer: what each slot holds, and what must be done to it
    /// before this step.
    Changer { lines: Vec<RackLine>, unplaced: Vec<String>, clipped: usize, manual_changes: usize },
    /// A machine without one: every tool is put in by hand, in this order.
    Manual { sequence: Vec<String> },
    /// The step has no tooling solution; the reasons.
    Failed(Vec<String>),
    /// The step loads no tools.
    Nothing,
}

/// One slot of a tool changer.
#[derive(Clone, Debug, PartialEq)]
pub struct RackLine {
    pub slot: String,
    pub tool: String,
    pub action: String,
}

/// One tool block, in the order the program runs them.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolRow {
    pub slot: String,
    pub tool: String,
    pub diameter: String,
    pub feed: String,
    pub plunge: String,
    pub spindle: String,
    pub depth: String,
    pub ops: usize,
}

/// Builds the sheet.
pub fn build(sources: &Sources) -> SetupSheet {
    let unit = sources.unit;
    let len = |length: Length| format_length_display(length, unit);
    let board_name = sources.board.map(|b| b.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Untitled board".into());

    SetupSheet {
        board_name,
        profile_name: sources.profile_name.clone(),
        generated: sources.generated.clone(),
        version: sources.version.clone(),
        board: board_facts(sources, &len),
        holes: sources.board.map(|board| hole_rows(board, &len)).unwrap_or_default(),
        engraving: sources
            .engraving
            .iter()
            .enumerate()
            .filter_map(|(index, facts)| Some(engraving_summary(index, sources.steps.get(index)?, facts.as_ref()?, &len)))
            .collect(),
        steps: sources.steps.iter().enumerate().map(|(index, raw)| step_sheet(sources, index, raw)).collect(),
    }
}

/// The board section's one-line facts.
fn board_facts(sources: &Sources, len: &dyn Fn(Length) -> String) -> Vec<(String, String)> {
    let Some(board) = sources.board else {
        return vec![("Board".into(), "No board loaded".into())];
    };
    let mut facts = Vec::new();
    if let Some(bbox) = board.bounding_box.as_ref() {
        facts.push(("Size".into(), format!("{} × {}", len(bbox.width), len(bbox.height))));
    }
    if let Some(thickness) = board.thickness {
        facts.push(("Thickness".into(), len(thickness)));
    }
    let copper: Vec<String> = [(pcb::FRONT_COPPER, "front"), (pcb::BACK_COPPER, "back")]
        .iter()
        .filter_map(|(layer, face)| board.copper_thickness.get(layer).map(|t| format!("{} {face}", len(*t))))
        .collect();
    if !copper.is_empty() {
        facts.push(("Copper".into(), copper.join(", ")));
    }
    if let Some(layers) = sources.engraving.iter().flatten().filter_map(|e| e.held.as_ref()).map(|h| h.copper_layers).find(|n| *n > 0) {
        facts.push(("Copper layers".into(), layers.to_string()));
    }

    let count = |kind: HoleKind| board.holes.iter().filter(|h| h.kind == kind).count();
    facts.push((
        "Holes".into(),
        format!("{} in all: {} vias, {} plated (PTH), {} non-plated (NPTH)", board.holes.len(), count(HoleKind::Via), count(HoleKind::PadPth), count(HoleKind::PadNpth)),
    ));
    let slots = board.holes.iter().filter(|h| h.slot().is_some()).count();
    if slots > 0 {
        facts.push(("Slots".into(), format!("{slots} oblong holes, routed")));
    }
    if let Some(stitched) = sources.stitched.filter(|s| s.errors.is_empty()) {
        let cutouts = stitched.contours.iter().filter(|c| c.is_hole).count();
        let outlines = stitched.contours.len() - cutouts;
        facts.push(("Outline".into(), format!("{outlines} outline{}, {cutouts} interior cut-out{}", plural(outlines), plural(cutouts))));
    }
    facts
}

/// One row per distinct drilled size and per distinct slot, smallest first within each kind.
fn hole_rows(board: &BoardSnapshot, len: &dyn Fn(Length) -> String) -> Vec<Vec<String>> {
    // Keyed in whole microns, the precision the assigner compares diameters at.
    let mut groups: BTreeMap<(u8, i64, i64), (Length, Option<Length>, usize)> = BTreeMap::new();
    for hole in &board.holes {
        let kind = match hole.kind {
            HoleKind::Via => 0,
            HoleKind::PadPth => 1,
            HoleKind::PadNpth => 2,
        };
        let (size, width) = match hole.slot() {
            Some(slot) => (slot.length, Some(slot.width)),
            None => match hole.drill_axes() {
                Some((major, _)) => (major, None),
                None => continue,
            },
        };
        let key = (kind, (size.as_mm() * 1000.0).round() as i64, width.map_or(-1, |w| (w.as_mm() * 1000.0).round() as i64));
        groups.entry(key).or_insert((size, width, 0)).2 += 1;
    }
    groups
        .into_iter()
        .map(|((kind, _, _), (size, width, count))| {
            let kind = ["Via", "PTH", "NPTH"][kind as usize];
            let size = match width {
                Some(width) => format!("slot {} × {}", len(size), len(width)),
                None => format!("⌀{}", len(size)),
            };
            vec![kind.to_string(), size, count.to_string()]
        })
        .collect()
}

fn engraving_summary(index: usize, raw: &StepRaw, facts: &EngraveFacts, len: &dyn Fn(Length) -> String) -> EngravingSummary {
    let face = if facts.back { "back" } else { "front" };
    let mut lines = vec![
        ("Tool".to_string(), facts.tool.clone()),
        ("Channel".to_string(), format!("{} (at least {} asked for)", len(facts.achieved), len(facts.requested))),
    ];
    match &facts.held {
        None => lines.push(("Contours".into(), "Still being worked out when the sheet was made".into())),
        Some(held) => {
            lines.push((
                "Contours".into(),
                format!("{} contours, {} of isolation", held.contours, len(Length::from_mm(held.length_nm / 1e6))),
            ));
            if held.narrowed > 0 {
                lines.push(("Narrowed".into(), format!("{} stretches narrowed to fit between close nets", held.narrowed)));
            }
            if held.uncut_nm > 0.0 {
                lines.push(("Not separated".into(), format!("{} of copper no cut separates", len(Length::from_mm(held.uncut_nm / 1e6)))));
            }
            if held.islands_removed > 0 {
                lines.push(("Islands".into(), format!("{} removed", held.islands_removed)));
            }
            if held.milled_pieces > 0 {
                lines.push(("Cleared".into(), format!("{} pieces of free copper cleared by the mill", held.milled_pieces)));
            }
        }
    }
    EngravingSummary { title: format!("Step {}: {} — {face} copper", index + 1, raw.name), facts: lines }
}

fn step_sheet(sources: &Sources, index: usize, raw: &StepRaw) -> StepSheet {
    let unit = sources.unit;
    let len = |length: Length| format_length_display(length, unit);
    let machine = raw.cnc_id.and_then(|id| sources.machines.iter().find(|m| m.id == id.to_string()));
    let fixture = raw.fixture_id.and_then(|id| sources.fixtures.iter().find(|f| f.id == id.to_string()));
    let toolset = raw.toolset_id.and_then(|id| sources.toolsets.iter().find(|t| t.id == id.to_string()));
    let plan_step = sources.plan.steps.iter().find(|step| step.index == index);

    let machine_text = match machine {
        Some(m) if m.atc_slot_count > 0 => format!("{} · automatic tool changer, {} slots", m.name, m.atc_slot_count),
        Some(m) => format!("{} · manual tool changes", m.name),
        None => "No CNC bound".to_string(),
    };

    let program_file = sources.program_files.get(&index).cloned().unwrap_or_default();
    let program = match sources.programs.get(index) {
        Some(step) => match (step.program(), step.failure()) {
            (Some(program), _) if !program_file.is_empty() => format!("{program_file} · {} lines", program.text.lines().count()),
            (Some(program), _) => format!("{} lines — not included in this export", program.text.lines().count()),
            (None, Some(failure)) => format!("No program: {failure}"),
            (None, None) => "No program".to_string(),
        },
        None => "No program".to_string(),
    };

    let mut notes: Vec<String> = plan_step.map(|step| step.notes.clone()).unwrap_or_default();
    if let Some(StepOutcome::Resolved(resolved)) = sources.tooling.steps.get(index).map(|s| &s.outcome) {
        notes.extend(resolved.warnings.iter().cloned());
    }

    StepSheet {
        number: index + 1,
        name: raw.name.clone(),
        back: raw.machines_back,
        face: if raw.machines_back { "Back".into() } else { "Front".into() },
        machine: machine_text,
        work: kinds_of_work(&raw.operations),
        operations: raw.operations.iter().map(|op| describe_operation(op, raw, &len)).collect(),
        setup: setup_facts(raw, machine, fixture, toolset, &len),
        rack: rack_section(sources, index, plan_step),
        tools: plan_step
            .map(|step| tool_rows(sources, step, sources.limits.get(index).copied(), &len))
            .unwrap_or_default(),
        notes,
        program,
        program_file,
    }
}

/// What a step does, in a word or two each, in the order a board is made.
fn kinds_of_work(operations: &[String]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for op in operations {
        let kind = match op.as_str() {
            "engrave_copper" => "Isolation",
            "drill_locating_pins" => "Locating pins",
            "drill_pth" | "drill_npth" => "Drilling",
            "route_cutouts" | "route_board" => "Routing",
            _ => continue,
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() { "Nothing".to_string() } else { kinds.join(", ") }
}

/// An operation's name, with the settings an operator setting up the machine would check.
fn describe_operation(key: &str, raw: &StepRaw, len: &dyn Fn(Length) -> String) -> String {
    let label = operation_label(key).to_string();
    match key {
        "engrave_copper" => {
            let e = &raw.engrave_copper;
            let mut parts = vec![format!("channel at least {}", len(e.width))];
            if e.test_cut {
                parts.push("depth test cut first".into());
            }
            parts.push(if e.remove_islands { "islands removed".into() } else { "islands left".into() });
            if e.clear_narrow_copper {
                parts.push(format!("copper narrower than {} cleared", len(e.clearing_threshold)));
            }
            format!("{label}: {}", parts.join(", "))
        }
        "route_board" => {
            let edge = &raw.route_board;
            let tabs = if edge.outline.tabs {
                format!(
                    "{} tab{} {} wide{}",
                    edge.outline.count,
                    plural(edge.outline.count),
                    len(edge.outline.width),
                    if edge.outline.mouse_bites { ", mouse bites" } else { "" },
                )
            } else {
                "no tabs".into()
            };
            format!("{label}: {} cutter, {tabs}", len(edge.kerf))
        }
        "route_cutouts" => {
            let held = if raw.route_cutouts.retain_island { "slugs held by tabs" } else { "slugs cut free" };
            format!("{label}: {held}")
        }
        "drill_locating_pins" => match raw.pin_diameter {
            Some(d) => format!("{label}: ⌀{}", len(d)),
            None => label,
        },
        _ => label,
    }
}

fn setup_facts(
    raw: &StepRaw,
    machine: Option<&MachineProfile>,
    fixture: Option<&FixtureProfile>,
    toolset: Option<&ToolsetProfile>,
    len: &dyn Fn(Length) -> String,
) -> Vec<(String, String)> {
    let name = |n: Option<&str>| n.map(str::to_string).unwrap_or_else(|| "Not bound".into());
    let mut facts = vec![
        ("CNC".to_string(), name(machine.map(|m| m.name.as_str()))),
        ("Fixture".to_string(), name(fixture.map(|f| f.name.as_str()))),
        ("Toolset".to_string(), name(toolset.map(|t| t.name.as_str()))),
    ];
    if let Some(fixture) = fixture {
        let x = if fixture.origin_x0.eq_ignore_ascii_case("right") { "right" } else { "left" };
        let y = if fixture.origin_y0.eq_ignore_ascii_case("far") { "far" } else { "near" };
        let reference = if fixture.origin_reference.trim().is_empty() { String::new() } else { format!(", stored as {}", fixture.origin_reference.trim()) };
        facts.push(("Work zero".into(), format!("{y}-{x} corner{reference}")));
        facts.push((
            "Clearance".into(),
            format!("X {}, Y {} between the zero and the nearest cut", len(fixture.work_clearance_x), len(fixture.work_clearance_y)),
        ));
    }
    facts.push(("Z zero".into(), "Top of the board".into()));
    if raw.machines_back {
        let turn = match fixture.map(|f| crate::gcode::placement::BoardFlip::from_axis(&f.board_flip_axis)) {
            Some(flip) if flip.mirrors_x() => "About Y, like a page: left and right swap".to_string(),
            Some(_) => "About X, near to far: near and far swap".to_string(),
            None => "Not known: the step has no fixture".to_string(),
        };
        facts.push(("Turn the board over".into(), turn));
        facts.push(("The program asks".into(), format!("“{}” before it moves", crate::runtime::BACK_FACE_PROMPT)));
    }
    if let Some(pin) = raw.pin_diameter {
        facts.push(("Locating pins".into(), format!("⌀{}", len(pin))));
    }
    facts
}

fn rack_section(sources: &Sources, index: usize, plan_step: Option<&StepPlan>) -> RackSection {
    match sources.tooling.steps.get(index).map(|s| &s.outcome) {
        Some(StepOutcome::Failed(reasons)) => return RackSection::Failed(reasons.clone()),
        Some(StepOutcome::Empty) | None => return RackSection::Nothing,
        Some(StepOutcome::Resolved(_)) => {}
    }
    match sources.tooling.rack_for_step(index) {
        Some(view) => changer_section(&view),
        None => {
            // No changer: each tool block is a tool the operator puts in by hand, when the
            // program stops and asks for it.
            let sequence: Vec<String> = plan_step
                .map(|step| {
                    step.blocks
                        .iter()
                        .map(|block| {
                            let tool = tool_name(sources.tools, &block.tool_id);
                            match block.slot {
                                Some(slot) => format!("T{slot}: {tool}"),
                                None => tool,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            if sequence.is_empty() { RackSection::Nothing } else { RackSection::Manual { sequence } }
        }
    }
}

/// A changer's slots for one step, with what the operator does to each.
fn changer_section(view: &RackStepView) -> RackSection {
    let lines: Vec<RackLine> = view
        .slots
        .iter()
        .filter(|slot| slot.status != SlotChange::Empty)
        .map(|slot| RackLine {
            slot: slot.slot.clone(),
            tool: slot.tool.clone().unwrap_or_default(),
            action: match slot.status {
                SlotChange::Load => "Load before this step",
                SlotChange::Kept => "Already loaded",
                SlotChange::Fixed => "Fixed by the toolset",
                SlotChange::Empty => "",
            }
            .to_string(),
        })
        .collect();
    let loads = view.slots.iter().filter(|slot| slot.status == SlotChange::Load).count();
    RackSection::Changer {
        lines,
        unplaced: view.unplaced.clone(),
        clipped: view.clipped_slots,
        manual_changes: loads + view.unplaced.len(),
    }
}

fn tool_name(tools: &[Tool], id: &str) -> String {
    tools.iter().find(|t| t.id == id).map(Tool::display_name).unwrap_or_else(|| format!("Unknown tool ({id})"))
}

/// One row per tool block, with the feeds the program runs it at.
///
/// The feeds come from [`feeds::resolve`] over the same tool values and machine limits the
/// program renderer uses (`AppCtx::tool_feed_map`, `AppCtx::step_machine_limits`), so the
/// sheet prints what the program emits — a clamped spindle and all.
fn tool_rows(sources: &Sources, step: &StepPlan, limits: Option<MachineLimits>, len: &dyn Fn(Length) -> String) -> Vec<ToolRow> {
    let unit = sources.unit;
    step.blocks
        .iter()
        .map(|block| {
            let resolved = sources
                .tool_feeds
                .get(&block.tool_id)
                .zip(limits)
                .and_then(|(tf, limits)| feeds::resolve(tf.feeds, tf.speed, limits).ok());
            let depth = block
                .ops
                .iter()
                .map(|op| op.z.z_bottom.as_mm())
                .fold(f64::INFINITY, f64::min);
            ToolRow {
                slot: block.slot.map(|s| format!("T{s}")).unwrap_or_else(|| "—".into()),
                tool: tool_name(sources.tools, &block.tool_id),
                diameter: format!("⌀{}", len(block.diameter)),
                feed: resolved.map(|fs| format_feed_display(fs.table, unit)).unwrap_or_else(|| "—".into()),
                plunge: resolved.map(|fs| format_feed_display(fs.z, unit)).unwrap_or_else(|| "—".into()),
                spindle: resolved.map(|fs| format_rotational_speed_display(fs.rpm)).unwrap_or_else(|| "—".into()),
                // Machine Z is negative into the work; the operator reads a depth.
                depth: if depth.is_finite() { len(Length::from_mm(-depth)) } else { "—".into() },
                ops: block.op_count(),
            }
        })
        .collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcode::feeds::SpindleRange;
    use crate::gcode::plan::{AtomicOp, OpKind, Phase, Point, ToolBlock, ZProfile};
    use crate::runtime::tooling::{RackSlotView, StepPlan as ToolingStep, StepResolved};
    use pcb::{BoardHole, BoardPoint};
    use units::{FeedRate, RotationalSpeed};
    use uuid::Uuid;

    fn hole(kind: HoleKind, x_mm: f64, y_mm: f64) -> BoardHole {
        BoardHole {
            id: None,
            kind,
            position: BoardPoint { x: Length::from_mm(0.0), y: Length::from_mm(0.0) },
            drill_x: Some(Length::from_mm(x_mm)),
            drill_y: Some(Length::from_mm(y_mm)),
            plated: None,
            orientation_deg: None,
        }
    }

    fn board(holes: Vec<BoardHole>) -> BoardSnapshot {
        BoardSnapshot {
            name: "panel.kicad_pcb".into(),
            thickness: Some(Length::from_mm(1.6)),
            copper_thickness: Default::default(),
            bounding_box: Some(pcb::BoardBoundingBox {
                x: Length::from_mm(0.0),
                y: Length::from_mm(0.0),
                width: Length::from_mm(50.0),
                height: Length::from_mm(30.0),
            }),
            edge_shapes: Vec::new(),
            holes,
        }
    }

    fn raw(operations: &[&str], back: bool) -> StepRaw {
        StepRaw {
            name: "Step".into(),
            operations: operations.iter().map(|s| s.to_string()).collect(),
            cnc_id: None,
            fixture_id: None,
            toolset_id: None,
            drill: Default::default(),
            route_board: Default::default(),
            route_cutouts: Default::default(),
            engrave_copper: Default::default(),
            machines_back: back,
            pin_diameter: None,
            router_depth_per_pass: Length::from_mm(1.0),
        }
    }

    fn fixture(id: &str, flip_axis: &str) -> FixtureProfile {
        FixtureProfile {
            id: id.into(),
            name: "Pinned".into(),
            backing_board: "clamps".into(),
            backboard_thickness: Length::from_mm(2.5),
            bed_clearance: Length::from_mm(0.5),
            breakthrough: Length::from_mm(0.5),
            z_retract: Length::from_mm(5.0),
            z_safe: Length::from_mm(20.0),
            origin_x0: "left".into(),
            origin_y0: "near".into(),
            work_clearance_x: Length::from_mm(5.0),
            work_clearance_y: Length::from_mm(5.0),
            board_flip_axis: flip_axis.into(),
            origin_reference: "G55".into(),
            pending_required_fields: Default::default(),
            usable: true,
        }
    }

    fn op(depth_mm: f64) -> AtomicOp {
        let at = Point::new(Length::from_mm(0.0), Length::from_mm(0.0));
        AtomicOp {
            phase: Phase::Drill,
            kind: OpKind::Drill,
            tool_id: "t1".into(),
            entry: at,
            exit: at,
            z: ZProfile { z_bottom: Length::from_mm(-depth_mm), z_retract: Length::from_mm(2.0), z_feed: None, pass_depth: None },
            primitive: "drill",
            source: "pth#0".into(),
            continues_from_previous: false,
        }
    }

    fn block(tool_id: &str, slot: u8) -> ToolBlock {
        ToolBlock {
            tool_id: tool_id.into(),
            slot: Some(slot),
            diameter: Length::from_mm(0.8),
            ops: vec![op(1.8), op(2.0)],
            travel_mm: 0.0,
            verify_stop: None,
        }
    }

    fn resolved_step() -> ToolingStep {
        ToolingStep {
            name: "Step".into(),
            outcome: StepOutcome::Resolved(StepResolved {
                summary: String::new(),
                rack: Vec::new(),
                requirements: Vec::new(),
                warnings: vec!["Spindle clamped".into()],
                tool_ids: Vec::new(),
                cnc_name: String::new(),
            }),
            fault_places: Vec::new(),
        }
    }

    fn empty_tooling() -> ToolingPlan {
        ToolingPlan { steps: Vec::new(), note: None, racks: Vec::new() }
    }

    /// The inputs a test varies; everything else is empty.
    #[derive(Default)]
    struct Given<'a> {
        board: Option<&'a BoardSnapshot>,
        steps: Vec<StepRaw>,
        tooling: Option<ToolingPlan>,
        plan: MachiningPlan,
        fixtures: Vec<FixtureProfile>,
        feeds: BTreeMap<String, ToolFeed>,
        limits: Vec<MachineLimits>,
    }

    fn sheet(given: Given) -> SetupSheet {
        let files = BTreeMap::new();
        let tooling = given.tooling.unwrap_or_else(empty_tooling);
        build(&Sources {
            board: given.board,
            stitched: None,
            profile_name: "Two-sided".into(),
            generated: "2026-10-18 17:00".into(),
            version: "0.16.0".into(),
            unit: UserUnitSystem::Metric,
            steps: &given.steps,
            tooling: &tooling,
            plan: &given.plan,
            programs: &[],
            program_files: &files,
            engraving: vec![None; given.steps.len()],
            limits: given.limits,
            machines: &[],
            fixtures: &given.fixtures,
            toolsets: &[],
            tools: &[],
            tool_feeds: &given.feeds,
        })
    }

    /// The board's holes are counted by kind, and the size table has one row per distinct
    /// size, slots apart from drills.
    #[test]
    fn holes_are_counted_by_kind_and_size() {
        let board = board(vec![
            hole(HoleKind::Via, 0.3, 0.3),
            hole(HoleKind::Via, 0.3, 0.3),
            hole(HoleKind::PadPth, 0.8, 0.8),
            hole(HoleKind::PadNpth, 3.2, 3.2),
            hole(HoleKind::PadPth, 2.0, 1.0),
        ]);
        let sheet = sheet(Given { board: Some(&board), ..Given::default() });

        let holes = sheet.board.iter().find(|(key, _)| key == "Holes").map(|(_, v)| v.as_str());
        assert_eq!(holes, Some("5 in all: 2 vias, 2 plated (PTH), 1 non-plated (NPTH)"));
        assert!(sheet.board.iter().any(|(key, value)| key == "Slots" && value.starts_with("1 ")));
        assert_eq!(sheet.holes.iter().filter(|row| row[0] == "Via").count(), 1, "two vias of one size are one row");
        assert!(sheet.holes.iter().any(|row| row[1].starts_with("slot ")), "{:?}", sheet.holes);
        assert!(sheet.holes.iter().any(|row| row[1].starts_with('⌀')), "drills carry the diameter sign");
    }

    /// A back step says it is the back, which way the board turns over (from its own
    /// fixture), and the question the program will ask before it moves.
    #[test]
    fn a_back_step_says_how_the_board_turns_over() {
        let mut step = raw(&["drill_pth"], true);
        step.fixture_id = Some(Uuid::nil());
        let sheet = sheet(Given {
            steps: vec![step],
            fixtures: vec![fixture(&Uuid::nil().to_string(), "y")],
            ..Given::default()
        });

        let step = &sheet.steps[0];
        assert_eq!(step.face, "Back");
        let fact = |key: &str| step.setup.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(fact("Turn the board over").as_deref(), Some("About Y, like a page: left and right swap"));
        assert!(fact("The program asks").is_some_and(|v| v.contains(crate::runtime::BACK_FACE_PROMPT)));
        assert_eq!(fact("Work zero").as_deref(), Some("near-left corner, stored as G55"));
    }

    /// A tool changer's slots are listed with what to do to each, straight from the rack
    /// schedule: a slot to load is a manual change, as is a tool the rack cannot hold.
    #[test]
    fn a_changer_lists_what_to_do_to_each_slot() {
        let view = RackStepView {
            cnc_name: "Masso".into(),
            step_count: 2,
            slots: vec![
                RackSlotView { slot: "T1".into(), tool: Some("0.8 drill".into()), status: SlotChange::Kept },
                RackSlotView { slot: "T2".into(), tool: Some("1.0 drill".into()), status: SlotChange::Load },
                RackSlotView { slot: "T3".into(), tool: Some("probe".into()), status: SlotChange::Fixed },
                RackSlotView { slot: "T4".into(), tool: None, status: SlotChange::Empty },
            ],
            unplaced: vec!["2.0 router".into()],
            clipped_slots: 0,
        };
        let RackSection::Changer { lines, manual_changes, .. } = changer_section(&view) else {
            panic!("a changer section");
        };
        let actions: Vec<(&str, &str)> = lines.iter().map(|l| (l.slot.as_str(), l.action.as_str())).collect();
        assert_eq!(
            actions,
            vec![("T1", "Already loaded"), ("T2", "Load before this step"), ("T3", "Fixed by the toolset")],
            "empty slots are left out"
        );
        assert_eq!(manual_changes, 2, "one slot to load and one tool the rack cannot hold");
    }

    /// Without a changer, every tool goes in by hand, in the order the program runs them.
    #[test]
    fn without_a_changer_the_tools_go_in_by_hand_in_run_order() {
        let plan = MachiningPlan {
            steps: vec![StepPlan { index: 0, name: "Step".into(), blocks: vec![block("b", 2), block("a", 1)], notes: vec![] }],
            note: None,
        };
        let sheet = sheet(Given {
            steps: vec![raw(&["drill_pth"], false)],
            tooling: Some(ToolingPlan { steps: vec![resolved_step()], note: None, racks: Vec::new() }),
            plan,
            ..Given::default()
        });

        let RackSection::Manual { sequence } = &sheet.steps[0].rack else {
            panic!("a manual section: {:?}", sheet.steps[0].rack);
        };
        assert_eq!(sequence.len(), 2);
        assert!(sequence[0].starts_with("T2") && sequence[1].starts_with("T1"), "run order, not slot order: {sequence:?}");
        assert!(sheet.steps[0].notes.contains(&"Spindle clamped".to_string()), "tooling warnings are notes");
    }

    /// The tool table prints the feeds the program emits: resolved against the machine's
    /// limits, so a tool rated above the spindle's top speed shows the clamped speed.
    #[test]
    fn the_tool_table_shows_the_feeds_the_program_runs() {
        let rated = ToolFeed {
            name: "0.8 drill".into(),
            feeds: feeds::RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), Some(FeedRate::from_mm_per_min(300.0))),
            speed: Some(RotationalSpeed::from_rpm(30_000.0)),
        };
        let limits = MachineLimits {
            spindle: SpindleRange::new(RotationalSpeed::from_rpm(1_000.0), RotationalSpeed::from_rpm(24_000.0)),
            max_feed_xy: FeedRate::from_mm_per_min(5_000.0),
            max_feed_z: FeedRate::from_mm_per_min(5_000.0),
        };
        let mut feeds_map = BTreeMap::new();
        feeds_map.insert("t1".to_string(), rated.clone());
        let plan = MachiningPlan {
            steps: vec![StepPlan { index: 0, name: "Step".into(), blocks: vec![block("t1", 1)], notes: vec![] }],
            note: None,
        };
        let sheet = sheet(Given {
            steps: vec![raw(&["drill_pth"], false)],
            plan,
            feeds: feeds_map,
            limits: vec![limits],
            ..Given::default()
        });

        let expected = feeds::resolve(rated.feeds, rated.speed, limits).unwrap();
        let row = &sheet.steps[0].tools[0];
        assert_eq!(row.spindle, format_rotational_speed_display(expected.rpm), "the program's own resolution");
        assert_eq!(row.spindle, format_rotational_speed_display(RotationalSpeed::from_rpm(24_000.0)), "clamped");
        assert_eq!(row.feed, format_feed_display(expected.table, UserUnitSystem::Metric));
        assert_eq!(row.depth, format_length_display(Length::from_mm(2.0), UserUnitSystem::Metric), "the deepest op");
        assert_eq!(row.ops, 2);
    }

    /// Every step's section is laid out on a page of its own, after the job and board pages.
    #[test]
    fn each_step_starts_its_own_page() {
        let sheet = sheet(Given { steps: vec![raw(&["drill_pth"], false), raw(&["route_board"], true)], ..Given::default() });
        let blocks = crate::report::blocks(&sheet);
        let breaks = blocks.iter().filter(|b| matches!(b, crate::report::layout::Block::PageBreak)).count();
        assert_eq!(breaks, 2, "one per step");
        assert_eq!(sheet.steps[1].work, "Routing");
    }

    /// Writes a sample sheet to the path in `K2G_SAMPLE_SHEET`, to look at by eye — layout is
    /// the one thing about a PDF the other tests cannot judge. Ignored, so it only runs when
    /// asked: `K2G_SAMPLE_SHEET=sheet.pdf cargo test -p k2g sample_sheet -- --ignored`.
    #[test]
    #[ignore]
    fn sample_sheet() {
        let Ok(path) = std::env::var("K2G_SAMPLE_SHEET") else { return };
        let board = board(vec![
            hole(HoleKind::Via, 0.3, 0.3),
            hole(HoleKind::PadPth, 0.8, 0.8),
            hole(HoleKind::PadPth, 1.0, 1.0),
            hole(HoleKind::PadNpth, 3.2, 3.2),
            hole(HoleKind::PadPth, 2.0, 1.0),
        ]);
        let mut back = raw(&["engrave_copper", "drill_pth", "drill_npth", "route_board"], true);
        back.name = "Back: isolate, drill and cut out".into();
        back.fixture_id = Some(Uuid::nil());
        let mut front = raw(&["drill_locating_pins", "engrave_copper"], false);
        front.name = "Front: pins and isolation".into();
        front.pin_diameter = Some(Length::from_mm(3.0));
        front.fixture_id = Some(Uuid::nil());
        let plan = MachiningPlan {
            steps: vec![
                StepPlan { index: 0, name: "Front".into(), blocks: vec![block("t1", 1), block("t2", 2)], notes: vec!["The depth test cut runs first; check it before the program goes on.".into()] },
                StepPlan { index: 1, name: "Back".into(), blocks: vec![block("t1", 1), block("t2", 2), block("t3", 3)], notes: vec![] },
            ],
            note: None,
        };
        let sheet = sheet(Given {
            board: Some(&board),
            steps: vec![front, back],
            tooling: Some(ToolingPlan { steps: vec![resolved_step(), resolved_step()], note: None, racks: Vec::new() }),
            plan,
            fixtures: vec![fixture(&Uuid::nil().to_string(), "y")],
            ..Given::default()
        });
        let fonts = crate::report::pdf::Fonts::load().unwrap();
        let pages = crate::report::layout::layout(&fonts, &crate::report::blocks(&sheet));
        let chrome = crate::report::pdf::Chrome {
            header: "panel.kicad_pcb · Two-sided".into(),
            generated: "2026-10-18 17:00".into(),
            producer: "Setup sheet · k2g 0.16.0".into(),
            title: "Setup sheet".into(),
        };
        std::fs::write(path, crate::report::pdf::write(&fonts, &pages, &chrome).unwrap()).unwrap();
    }
}
