use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::job::ProductionOperation;
use super::state::RackSlot;

/// CNC profile persisted with the CNC schema.
#[derive(Clone)]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
    pub built_in: bool,
    /// Fixture plate X travel, stored in mm.
    pub fixture_plate_max_x: u32,
    /// Fixture plate Y travel, stored in mm.
    pub fixture_plate_max_y: u32,
    /// Maximum feed rate, stored in mm/min.
    pub max_feed_rate_mm_per_min: u32,
    pub spindle_min_rpm: u32,
    pub spindle_max_rpm: u32,
    pub atc_slot_count: u8,
    pub origin_x0: String,
    pub origin_y0: String,
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
    pub tool_change_manual_prompt: String,
    pub tool_change_command: String,
    pub pending_required_fields: BTreeSet<String>,
    pub usable: bool,
}

impl Default for MachineProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            built_in: false,
            fixture_plate_max_x: 300,
            fixture_plate_max_y: 200,
            max_feed_rate_mm_per_min: 2000,
            spindle_min_rpm: 3000,
            spindle_max_rpm: 24000,
            atc_slot_count: 0,
            origin_x0: "Left".to_string(),
            origin_y0: "Front".to_string(),
            scaling_x: 100.0,
            scaling_y: 100.0,
            line_numbering_increment: 10,
            gcode_header: concat!(
                "(Created by kicad2gcode from '{pcb_filename}' - {timestamp})\n",
                "(Reset all back to safe defaults)\n",
                "G17 G54 G40 G49 G80 G90\n",
                "G21\n",
                "G10 P0\n",
                "(Establish the Z-Safe)\n",
                "G0 Z{z_safe}"
            ).to_string(),
            gcode_footer: "(end of file)".to_string(),
            drill_first_move: "G0 X{x} Y{y} Z{z_safe}".to_string(),
            drill_cycle_mode_last: "G98".to_string(),
            drill_cycle_mode_series: "G99".to_string(),
            drill_cycle_start: "G81 Z{z_bottom} R{z_retract} F{z_feedrate}".to_string(),
            drill_next_hole: "X{x} Y{y}".to_string(),
            drill_cycle_cancel: "G80".to_string(),
            route_plunge_and_offset: "G90 G0 X{x} Y{y}\nG1 Z{z_bottom} F{z_feedrate}\nG1 Y{y_plus_id}".to_string(),
            route_arc_up: "G2 I0 J-{id}".to_string(),
            route_arc_down: "G3 I0 J-{id}".to_string(),
            route_retract: "G0 Z{z_safe}".to_string(),
            tool_change_manual_prompt: "MSG Load {tool_name} {tool_diameter}\nM01".to_string(),
            tool_change_command: "M05\n{manual_message}\nT{slot} M06\nS{rpm}".to_string(),
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
    pub fixture_profile_id: String,
    pub toolset_profile_id: String,
    pub default_operations: Vec<ProductionOperation>,
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
