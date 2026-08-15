use std::collections::BTreeMap;
use std::collections::BTreeSet;
use serde_json::Value;
use units::{FeedRate, Length, RotationalSpeed};

use super::job::{BoardFace, ProductionOperation};
use super::state::RackSlot;

/// CNC profile persisted with the CNC schema.
#[derive(Clone)]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
    pub spindle_rpm_min: RotationalSpeed,
    pub spindle_rpm_max: RotationalSpeed,
    /// The fastest the machine can feed in the XY plane. A tool rated faster is run at a
    /// lower spindle speed rather than merely fed slower, so its chip load still holds —
    /// see [`crate::gcode::feeds`].
    pub max_feed_xy: FeedRate,
    /// The same for Z. Kept separate because Z is usually the slower axis and drilling is
    /// entirely Z motion, so this is the limit that most often binds.
    pub max_feed_z: FeedRate,
    /// Extension a saved program takes, without the dot. The CNC's primitives decide the
    /// output format, so the name a program is saved under belongs with them — a step
    /// whose machine emits Excellon must not be written as `.nc`.
    pub output_file_extension: String,
    pub atc_slot_count: u8,
    /// How far the emitted path may depart from the true curve. One value for arc
    /// fitting and arc flattening alike, so a profile cannot be accurate in one and
    /// coarse in the other. A machine fact: 0.01 mm suits a PCB router
    /// and is absurd for a plasma table.
    pub curve_tolerance: Length,
    pub scaling_x: f32,
    pub scaling_y: f32,
    /// The `line_number` primitive; empty means the program is not numbered.
    pub line_format_tpl: String,
    /// The `set_unit` primitive — how this machine is told which unit system to work
    /// in, emitted by a template's `metric()`/`imperial()` call. Empty for a machine
    /// that has no unit statement.
    pub set_unit_tpl: String,
    /// The `set_origin` primitive — how this machine is told which stored zero to work
    /// from, emitted by a template's `set_origin()` call. It also *validates* the
    /// fixture's origin reference, since which offsets exist is a machine fact. Empty for
    /// a machine that selects no origin.
    pub set_origin_tpl: String,
    // The rest, each named for the primitive it carries. They were once
    // `gcode_header`, `drill_cycle_mode_series`, `route_arc_down` and the like — names
    // from a design that predates the primitives, so the struct said `route_retract`
    // for what is a comment and `drill_cycle_start` for what starts the spindle.
    pub program_begin_tpl: String,
    pub program_end_tpl: String,
    pub tool_change_tpl: String,
    /// Emitted after `tool_change`, but only on a machine whose
    /// `tool_length_measurement` is `manual` — an automatic setter measures at M06.
    pub tool_measure_tpl: String,
    pub spindle_start_tpl: String,
    pub spindle_stop_tpl: String,
    pub move_rapid_tpl: String,
    pub cut_linear_tpl: String,
    /// The Z-only feed move into the material. Blank falls back to `cut_linear` with
    /// x, y and z, which is what every profile emitted before this primitive existed.
    pub cut_plunge_tpl: String,
    pub cut_arc_tpl: String,
    pub drill_tpl: String,
    /// The operator callables. Nothing emits these — a template calls `comment("…")`,
    /// `message("…")` or `pause("…")`.
    pub comment_tpl: String,
    pub message_tpl: String,
    pub pause_tpl: String,
    /// Whether this machine needs a measurement block after each tool change, from
    /// `machine.tool_length_measurement`. Carried here so the renderer does not have to
    /// re-read the profile document to answer it.
    pub measures_tool_length: bool,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

impl Default for MachineProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            spindle_rpm_min: RotationalSpeed::from_rpm(0.0),
            spindle_rpm_max: RotationalSpeed::from_rpm(0.0),
            // Zero, not the schema's 5000: a `Default` profile is a placeholder that has
            // not been read from anywhere, and inventing a plausible machine limit here
            // would let an unconfigured CNC silently produce a program.
            max_feed_xy: FeedRate::from_mm_per_min(0.0),
            max_feed_z: FeedRate::from_mm_per_min(0.0),
            output_file_extension: String::new(),
            atc_slot_count: 0,
            curve_tolerance: Length::from_mm(0.01),
            scaling_x: 1.0,
            scaling_y: 1.0,
            line_format_tpl: String::new(),
            set_unit_tpl: String::new(),
            set_origin_tpl: String::new(),
            program_begin_tpl: String::new(),
            program_end_tpl: String::new(),
            tool_change_tpl: String::new(),
            tool_measure_tpl: String::new(),
            spindle_start_tpl: String::new(),
            spindle_stop_tpl: String::new(),
            move_rapid_tpl: String::new(),
            cut_linear_tpl: String::new(),
            cut_plunge_tpl: String::new(),
            cut_arc_tpl: String::new(),
            drill_tpl: String::new(),
            comment_tpl: String::new(),
            message_tpl: String::new(),
            pause_tpl: String::new(),
            measures_tool_length: false,
            pending_required_fields: BTreeSet::new(),
            usable: true,
        }
    }
}

/// Fixture profile persisted with fixture schema.
#[derive(Clone)]
pub struct FixtureProfile {
    pub id: String,
    pub name: String,
    pub backing_board: String,
    /// Thickness of the martyr/backing board under the PCB (`backboard_thickness`).
    pub backboard_thickness: Length,
    /// Minimum tip-to-bed clearance kept below the board (`bed_clearance`) — the
    /// Z-feasibility bed-safety limit.
    pub bed_clearance: Length,
    /// How far the tool passes below the board underside to fully clear a through
    /// feature (`breakthrough`), bounded by the backing board.
    pub breakthrough: Length,
    /// R-plane retract above the board top between features (`z_retract`).
    pub z_retract: Length,
    /// Safe travel height for rapids, clear of clamps and fixture hardware (`z_safe`).
    pub z_safe: Length,
    /// Which bed edge is X0 (`left`/`right`) and which is Y0 (`near`/`far`, near being
    /// the operator's side) — the corner the board is registered into in this fixture.
    ///
    /// The **bed's** directions, deliberately not the board's: the board's own faces are
    /// `front`/`back` ([`BoardFace`]), and one word meaning both is how a board comes off
    /// the machine mirrored. Kept as the schema's own words so the crosswalk stays a copy;
    /// `BoardOrigin::from_edges` interprets them.
    pub origin_x0: String,
    pub origin_y0: String,
    /// Which axis the board is turned about for a back-face step (`x`/`y`), in the
    /// schema's own words — `BoardFlip::from_axis` interprets them.
    ///
    /// A property of the fixture and of nothing else: it is decided entirely by where the
    /// registration is. Pins on a left-to-right line let the board be turned like a page
    /// (about Y, mirroring X); pins on a near-to-far line tumble it (about X).
    pub board_flip_axis: String,
    /// Which of the machine's stored zero points this fixture occupies, named the way the
    /// target machine names it (`G55`, or `G54.1 P7` on a MASSO). Held exactly as the
    /// operator entered it — normalising and validating it is the CNC profile's
    /// `set_origin` primitive's job, because which offsets exist is a machine fact.
    /// Empty means unset, which `set_origin` reports as an error rather than guessing.
    pub origin_reference: String,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

/// Machining profile projection (one setup / `steps[0]` today). Persisted with
/// the machining schema, whose ordered `steps` describe the full process.
#[derive(Clone)]
pub struct JobProfile {
    pub id: String,
    pub name: String,
    pub cnc_profile_id: String,
    pub fixture_profile_id: String,
    pub toolset_profile_id: String,
    pub board_face: BoardFace,
    pub default_operations: Vec<ProductionOperation>,
    pub operation_setups: BTreeMap<String, Value>,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

/// Toolset generation policy persisted with toolset schema.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolsetGenerationPolicy {
    FixedToolset,
    AllowReload,
    AllowHybrid,
}

impl ToolsetGenerationPolicy {
    pub fn as_key(self) -> &'static str {
        match self {
            Self::FixedToolset => "fixed_toolset",
            Self::AllowReload => "allow_reload",
            Self::AllowHybrid => "allow_hybrid",
        }
    }

    pub fn from_key(value: &str) -> Self {
        match value {
            "fixed_toolset" => Self::FixedToolset,
            "allow_reload" => Self::AllowReload,
            _ => Self::AllowHybrid,
        }
    }
}

/// Toolset profile persisted with toolset schema.
#[derive(Clone)]
pub struct ToolsetProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub generation_policy: ToolsetGenerationPolicy,
    pub slots: BTreeMap<u8, RackSlot>,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

/// Generic delete-impact payload used by profile screens.
#[derive(Clone, Default)]
pub struct CascadeDeleteImpact {
    pub primary_profiles: Vec<String>,
    pub dependent_process_profiles: Vec<String>,
    pub deleted_live_projects: Vec<String>,
}
