use std::collections::BTreeMap;
use std::collections::BTreeSet;
use serde_json::Value;
use units::{Length, RotationalSpeed};

use super::job::{ProductionOperation, Side};
use super::state::RackSlot;

/// CNC profile persisted with the CNC schema.
#[derive(Clone)]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
    pub spindle_rpm_min: RotationalSpeed,
    pub spindle_rpm_max: RotationalSpeed,
    pub atc_slot_count: u8,
    pub scaling_x: f32,
    pub scaling_y: f32,
    /// The `line_number` primitive; empty means the program is not numbered.
    pub line_number_tpl: String,
    /// How many stored zero points the controller holds — the count a fixture's
    /// `work_coordinate_system` indexes into.
    pub work_coordinate_systems: u8,
    pub gcode_header: String,
    pub gcode_footer: String,
    pub drill_first_move: String,
    pub drill_cycle_mode_series: String,
    pub drill_cycle_start: String,
    pub drill_next_hole: String,
    pub drill_cycle_cancel: String,
    pub route_plunge_and_offset: String,
    pub route_arc_up: String,
    pub route_arc_down: String,
    pub route_retract: String,
    pub tool_change_command: String,
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
            atc_slot_count: 0,
            scaling_x: 1.0,
            scaling_y: 1.0,
            line_number_tpl: String::new(),
            work_coordinate_systems: 1,
            gcode_header: "".to_string(),
            gcode_footer: "".to_string(),
            drill_first_move: "".to_string(),
            drill_cycle_mode_series: "".to_string(),
            drill_cycle_start: "".to_string(),
            drill_next_hole: "".to_string(),
            drill_cycle_cancel: "".to_string(),
            route_plunge_and_offset: "".to_string(),
            route_arc_up: "".to_string(),
            route_arc_down: "".to_string(),
            route_retract: "".to_string(),
            tool_change_command: "".to_string(),
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
    /// Which board edge is X0 (`left`/`right`) and which is Y0 (`front`/`back`) — the
    /// corner the board is registered against in this fixture. Kept as the schema's own
    /// words so the crosswalk stays a copy; `BoardOrigin::from_edges` interprets them.
    pub origin_x0: String,
    pub origin_y0: String,
    /// Which of the machine's stored zero points this fixture occupies, from 1. An
    /// ordinal: the CNC profile's `initialise` template names it (`G53 + n` on most
    /// controllers, `G54 + n` on a Bantam, whose G54 is reserved).
    pub work_coordinate_system: u8,
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
    pub side: Side,
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
