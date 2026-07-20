use std::collections::BTreeMap;
use std::collections::BTreeSet;
use serde_json::Value;
use units::{FeedRate, Length, RotationalSpeed};

use super::job::{CutDepthStrategy, ProductionOperation, Side};
use super::state::RackSlot;

/// CNC profile persisted with the CNC schema.
#[derive(Clone)]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
    pub max_feed_rate: FeedRate,
    pub spindle_rpm_min: RotationalSpeed,
    pub spindle_rpm_max: RotationalSpeed,
    pub atc_slot_count: u8,
    pub scaling_x: f32,
    pub scaling_y: f32,
    pub line_numbering_increment: u16,
    pub gcode_header: String,
    pub gcode_footer: String,
    pub drill_first_move: String,
    pub drill_cycle_mode_last: String,
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
            max_feed_rate: FeedRate::from_mm_per_min(0.0),
            spindle_rpm_min: RotationalSpeed::from_rpm(0.0),
            spindle_rpm_max: RotationalSpeed::from_rpm(0.0),
            atc_slot_count: 0,
            scaling_x: 1.0,
            scaling_y: 1.0,
            line_numbering_increment: 0,
            gcode_header: "".to_string(),
            gcode_footer: "".to_string(),
            drill_first_move: "".to_string(),
            drill_cycle_mode_last: "".to_string(),
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
    pub coordinate_context: String,
    pub backing_board: String,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

/// Machining profile persisted with processing schema.
#[derive(Clone)]
pub struct JobProfile {
    pub id: String,
    pub name: String,
    pub cnc_profile_id: String,
    pub cnc_profile_choices: Vec<String>,
    pub fixture_profile_id: String,
    pub fixture_profile_choices: Vec<String>,
    pub toolset_profile_id: String,
    pub toolset_profile_choices: Vec<String>,
    pub side: Side,
    pub default_operations: Vec<ProductionOperation>,
    pub cut_depth_strategy: CutDepthStrategy,
    pub multi_pass_max_depth: Length,
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

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::FixedToolset => "Fixed toolset",
            Self::AllowReload => "Allow reload",
            Self::AllowHybrid => "Allow hybrid",
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
