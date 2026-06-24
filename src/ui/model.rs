use std::collections::{BTreeMap, BTreeSet};

use crate::board::BoardSnapshot;
use crate::catalog::init::{catalog_dir, parse_catalog_with_backfill};
use crate::catalog::types::{Catalog, ToolType};
use crate::catalog::CatalogManager;
use crate::units::{Angle, FeedRate, Length, RotationalSpeed, UserUnitDisplay, UserUnitSystem};
use super::persistence_state;

#[derive(Clone, PartialEq)]
pub struct UiLaunchData {
    pub env_vars: Vec<(String, String)>,
    pub env_summary: String,
    pub cli_args: Vec<String>,
    pub kicad_status: String,
    pub board_snapshot: Option<BoardSnapshot>,
    pub save_filename_override: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Job,
    CncProfiles,
    FixtureProfiles,
    JobProfiles,
    Stock,
    Catalog,
}

impl Screen {
    pub fn label(self) -> &'static str {
        match self {
            Self::Job => "Job",
            Self::CncProfiles => "CNC",
            Self::FixtureProfiles => "Fixture",
            Self::JobProfiles => "Job Profiles",
            Self::Stock => "Stock",
            Self::Catalog => "Catalog",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::CncProfiles => "cnc-profiles",
            Self::FixtureProfiles => "fixture-profiles",
            Self::JobProfiles => "job-profiles",
            Self::Stock => "stock",
            Self::Catalog => "catalog",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JobCenterView {
    Board,
    Machining,
    Code,
    Rack,
}

impl JobCenterView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Machining => "Machining",
            Self::Code => "Code",
            Self::Rack => "Rack",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Machining => "machining",
            Self::Code => "code",
            Self::Rack => "rack",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnitSystem {
    Metric,
    Imperial,
    Mil,
}

impl UnitSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Imperial => "imperial",
            Self::Mil => "mil",
        }
    }

    pub fn user_unit_system(self) -> UserUnitSystem {
        match self {
            Self::Metric => UserUnitSystem::Metric,
            Self::Imperial | Self::Mil => UserUnitSystem::Imperial,
        }
    }

    pub fn length_unit_label(self) -> &'static str {
        match self {
            Self::Metric => "mm",
            Self::Imperial => "in",
            Self::Mil => "mil",
        }
    }

    pub fn feed_unit_label(self) -> &'static str {
        match self {
            Self::Metric => "mm/min",
            Self::Imperial | Self::Mil => "in/min",
        }
    }

}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "light" => Self::Light,
            _ => Self::Dark,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GenerationState {
    Idle,
    Generating,
    Failed,
}

#[derive(Clone)]
pub struct MachineProfile {
    pub id: String,
    pub name: String,
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
    pub line_numbering_enabled: bool,
    pub line_numbering_increment: u32,
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
}

impl Default for MachineProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
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
            line_numbering_enabled: false,
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
        }
    }
}

#[derive(Clone)]
pub struct FixtureProfile {
    pub id: String,
    pub name: String,
    pub coordinate_context: String,
    pub backing_board: String,
}

#[derive(Clone)]
pub struct JobProfile {
    pub id: String,
    pub name: String,
    pub cnc_profile_id: String,
    pub fixture_profile_id: String,
}

#[derive(Clone, Default)]
pub struct CascadeDeleteImpact {
    pub primary_profiles: Vec<String>,
    pub dependent_job_profiles: Vec<String>,
    pub deleted_live_jobs: Vec<String>,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    InStock,
    OutOfStock,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::InStock => "In stock",
            Self::OutOfStock => "Out of stock",
        }
    }

    pub fn class_name(self) -> &'static str {
        match self {
            Self::InStock => "status-in-stock",
            Self::OutOfStock => "status-out-of-stock",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolPreference {
    Preferred,
    Neutral,
    NotPreferred,
}

impl ToolPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preferred => "Preferred",
            Self::Neutral => "Neutral",
            Self::NotPreferred => "Not preferred",
        }
    }

    pub fn class_name(self) -> &'static str {
        match self {
            Self::Preferred => "status-preferred",
            Self::Neutral => "status-neutral",
            Self::NotPreferred => "status-not-preferred",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct Tool {
    pub id: String,
    pub composite_name: String,
    pub name: String,
    pub kind: String,
    pub diameter: Length,
    pub catalog_diameter: Option<Length>,
    pub point_angle: Angle,
    pub catalog_point_angle: Option<Angle>,
    pub feed_rate: Option<FeedRate>,
    pub catalog_feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub catalog_spindle_speed: Option<RotationalSpeed>,
    pub status: ToolStatus,
    pub preference: ToolPreference,
    pub source_catalog: String,
    pub manufacturer: Option<String>,
    pub sku: Option<String>,
}

impl Tool {
    pub fn display_name(&self) -> String {
        let composite = self.composite_name.trim();
        let nickname = self.name.trim();

        if nickname.is_empty() {
            composite.to_string()
        } else {
            format!("{} - {}", composite, nickname)
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct CatalogStockTool {
    pub key: String,
    pub catalog_name: String,
    pub section_name: String,
    pub display_name: String,
    pub kind: String,
    pub diameter: Length,
    pub point_angle: Angle,
    pub feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub sku: Option<String>,
}

#[derive(Clone)]
pub struct CatalogStockSection {
    pub key: String,
    pub name: String,
    pub tools: Vec<CatalogStockTool>,
}

#[derive(Clone)]
pub struct CatalogStockCatalog {
    pub key: String,
    pub name: String,
    pub built_in: bool,
    pub sections: Vec<CatalogStockSection>,
}

#[allow(dead_code)]
pub fn load_stock_catalog_index() -> Vec<CatalogStockCatalog> {
    // Primary source: user catalog directory.  Files are parsed and validated
    // by CatalogManager; only valid catalogs are included.
    let mut source_catalogs: Vec<(String, Catalog, bool)> = Vec::new();

    if let (Ok(mut manager), Ok(dir)) = (CatalogManager::new(), catalog_dir()) {
        let _ = manager.load_dir(&dir);
        source_catalogs = manager
            .catalogs()
            .map(|(stem, catalog)| (stem.to_string(), catalog.clone(), false))
            .collect();
    }

    // Fallback to built-in catalogs if user dir load yields no valid entries.
    if source_catalogs.is_empty() {
        let sources = [
            ("kyocera".to_string(), include_str!("../../resources/catalogs/kyocera.yaml")),
            ("unionfab".to_string(), include_str!("../../resources/catalogs/unionfab.yaml")),
            ("generic".to_string(), include_str!("../../resources/catalogs/generic.yaml")),
        ];

        for (stem, text) in sources {
            if let Ok(catalog) = parse_catalog_with_backfill(text, &stem) {
                source_catalogs.push((stem, catalog, true));
            }
        }
    }

    let mut out = Vec::new();

    for (stem, catalog, built_in) in source_catalogs {
        let catalog_key = slug(&stem);
        let mut sections = Vec::new();

        for (section_idx, section) in catalog.sections.iter().enumerate() {
            let section_key = format!("{}::s{}", catalog_key, section_idx);
            let mut tools = Vec::new();

            for (tool_idx, tool) in section.tools.iter().enumerate() {
                let feed_rate = tool
                    .table_feed
                    .or(tool.z_feed);

                let kind = match tool.tool_type {
                    ToolType::Drillbit => "Drill",
                    ToolType::Routerbit => "Router",
                }
                .to_string();

                let display_name = if tool.sku_name.trim().is_empty() {
                    format!("{} {}", kind, tool.diameter.unit_display(UserUnitSystem::Metric).user)
                } else {
                    tool.sku_name.clone()
                };

                tools.push(CatalogStockTool {
                    key: format!("{}::t{}", section_key, tool_idx),
                    catalog_name: catalog.name.clone(),
                    section_name: section.name.clone(),
                    display_name,
                    kind,
                    diameter: tool.diameter,
                    point_angle: tool.point_angle,
                    feed_rate,
                    spindle_speed: tool.spindle_rpm,
                    sku: Some(tool.sku_name.clone()),
                });
            }

            sections.push(CatalogStockSection {
                key: section_key,
                name: section.name.clone(),
                tools,
            });
        }

        out.push(CatalogStockCatalog {
            key: catalog_key,
            name: catalog.name,
            built_in,
            sections,
        });
    }

    out
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        }
    }

    if out.is_empty() {
        "catalog".to_string()
    } else {
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RotationMode {
    Auto,
    Manual,
}

impl RotationMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtcRackStrategy {
    Off,
    Reuse,
    Overwrite,
}

impl AtcRackStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Reuse => "reuse",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProductionOperation {
    DrillPth,
    DrillNpth,
    MillBoard,
}

impl ProductionOperation {
    pub fn all() -> [Self; 3] {
        [Self::DrillPth, Self::DrillNpth, Self::MillBoard]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DrillPth => "Drill Plated Through Holes (PTH)",
            Self::DrillNpth => "Drill Non-Plated Through Holes (NPTH)",
            Self::MillBoard => "Mill Board Outline",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CutDepthStrategy {
    Automatic,
    SinglePass,
    MultiPass,
}

impl CutDepthStrategy {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::SinglePass => "single_pass",
            Self::MultiPass => "multi_pass",
        }
    }

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic (recommended)",
            Self::SinglePass => "Single Pass",
            Self::MultiPass => "Multi-pass",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardThicknessMode {
    Automatic,
    Preset,
    UserDefined,
    Probe,
}

impl BoardThicknessMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Preset => "preset",
            Self::UserDefined => "user_defined",
            Self::Probe => "probe",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Z0DeterminationMode {
    ManualAdjustZ0,
    TouchProbe,
}

impl Z0DeterminationMode {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualAdjustZ0 => "manual_adjust_z0",
            Self::TouchProbe => "touch_probe",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TouchProbeSource {
    ManualInstallation,
    AtcSlot,
}

impl TouchProbeSource {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManualInstallation => "manual_installation",
            Self::AtcSlot => "atc_slot",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardOrientation {
    Automatic,
    NoRotation,
    Rotate90,
    Rotate180,
    Rotate270,
    RotateCustom,
}

impl BoardOrientation {
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::NoRotation => "no_rotation",
            Self::Rotate90 => "rotate_90",
            Self::Rotate180 => "rotate_180",
            Self::Rotate270 => "rotate_270",
            Self::RotateCustom => "rotate_custom",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct JobConfig {
    pub selected_operations: Vec<ProductionOperation>,
    pub side: Side,
    pub rotation_mode: RotationMode,
    pub rotation_angle: i32,
    pub atc_strategy: AtcRackStrategy,
    pub tab_count: u8,
    pub tab_width_mm: f32,
    pub tab_width_baseline_mm: f32,
    pub allow_routing_holes: bool,
    pub drill_then_route: bool,
    pub pilot_hole_fallback: bool,
    pub cut_depth_strategy: CutDepthStrategy,
    pub multi_pass_max_depth_mm: f32,
    pub outline_router_tool_id: Option<String>,
    pub board_thickness_mode: BoardThicknessMode,
    pub board_thickness_preset_mm: f32,
    pub board_thickness_user_value: f32,
    pub z0_determination_mode: Z0DeterminationMode,
    pub touch_probe_source: TouchProbeSource,
    pub touch_probe_atc_slot: u8,
    pub mouse_bites_enabled: bool,
    pub mouse_bite_pitch_mm: f32,
    pub mouse_bite_drill_tool_id: Option<String>,
    pub board_orientation: BoardOrientation,
    pub board_orientation_custom_degrees: f32,
}

#[allow(dead_code)]
#[derive(Clone, PartialEq)]
pub struct AppError {
    pub id: String,
    pub is_error: bool,
    pub message: String,
    pub details: Option<String>,
}

#[derive(Clone)]
pub struct RackSlot {
    pub tool_id: Option<String>,
    pub locked: bool,
    pub disabled: bool,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct BoardLayers {
    pub holes: bool,
    pub routes: bool,
    pub paths: bool,
    pub tabs: bool,
}

#[derive(Clone)]
pub struct UiState {
    pub selected_screen: Screen,
    pub selected_job_view: JobCenterView,
    pub unit_system: UnitSystem,
    pub theme: Theme,
    pub machines: Vec<MachineProfile>,
    pub selected_machine_id: Option<String>,
    pub fixtures: Vec<FixtureProfile>,
    pub selected_fixture_id: Option<String>,
    pub job_profiles: Vec<JobProfile>,
    pub selected_job_profile_id: Option<String>,
    pub machine_mru: Vec<String>,
    pub focus_profile_name_editor: bool,
    pub catalogs: Vec<CatalogStockCatalog>,
    pub tools: Vec<Tool>,
    pub errors: Vec<AppError>,
    pub generation_state: GenerationState,
    pub job_config: JobConfig,
    pub gcode: String,
    pub save_filename: String,
    pub gcode_modified: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    #[allow(dead_code)]
    pub board_layers: BoardLayers,
    pub board: Option<BoardSnapshot>,
}

impl UiState {
    pub fn new(save_filename_override: Option<String>, board_snapshot: Option<BoardSnapshot>) -> Self {
        let tools = vec![
            Tool {
                id: "tool-1".to_string(),
                composite_name: "0.8mm Carbide Drill".to_string(),
                name: "Pilot holes".to_string(),
                kind: "Drill".to_string(),
                diameter: Length::from_mm(0.8),
                catalog_diameter: Some(Length::from_mm(0.8)),
                point_angle: Angle::from_degrees(130.0),
                catalog_point_angle: Some(Angle::from_degrees(130.0)),
                feed_rate: Some(FeedRate::from_mm_per_min(120.0)),
                catalog_feed_rate: Some(FeedRate::from_mm_per_min(120.0)),
                spindle_speed: Some(RotationalSpeed::from_rpm(18_000.0)),
                catalog_spindle_speed: Some(RotationalSpeed::from_rpm(18_000.0)),
                status: ToolStatus::InStock,
                preference: ToolPreference::Preferred,
                source_catalog: "CNCLab / Drills".to_string(),
                manufacturer: Some("CNCLab".to_string()),
                sku: Some("DRL-08".to_string()),
            },
            Tool {
                id: "tool-2".to_string(),
                composite_name: "1.0mm End Mill".to_string(),
                name: "Outline router".to_string(),
                kind: "End Mill".to_string(),
                diameter: Length::from_mm(1.0),
                catalog_diameter: Some(Length::from_mm(1.0)),
                point_angle: Angle::from_degrees(180.0),
                catalog_point_angle: Some(Angle::from_degrees(180.0)),
                feed_rate: Some(FeedRate::from_mm_per_min(280.0)),
                catalog_feed_rate: Some(FeedRate::from_mm_per_min(280.0)),
                spindle_speed: Some(RotationalSpeed::from_rpm(16_000.0)),
                catalog_spindle_speed: Some(RotationalSpeed::from_rpm(16_000.0)),
                status: ToolStatus::InStock,
                preference: ToolPreference::Neutral,
                source_catalog: "CNCLab / End Mills".to_string(),
                manufacturer: Some("CNCLab".to_string()),
                sku: Some("EM-10".to_string()),
            },
            Tool {
                id: "tool-3".to_string(),
                composite_name: "30deg V-Bit".to_string(),
                name: String::new(),
                kind: "V-Bit".to_string(),
                diameter: Length::from_mm(0.2),
                catalog_diameter: None,
                point_angle: Angle::from_degrees(30.0),
                catalog_point_angle: None,
                feed_rate: None,
                catalog_feed_rate: None,
                spindle_speed: None,
                catalog_spindle_speed: None,
                status: ToolStatus::OutOfStock,
                preference: ToolPreference::NotPreferred,
                source_catalog: "Manual".to_string(),
                manufacturer: None,
                sku: None,
            },
        ];

        let mut state = Self {
            selected_screen: Screen::Job,
            selected_job_view: JobCenterView::Board,
            unit_system: load_persisted_unit_system(),
            theme: load_persisted_theme(),
            machines: vec![],
            selected_machine_id: None,
            fixtures: vec![FixtureProfile {
                id: "fixture-default".to_string(),
                name: "Default fixture".to_string(),
                coordinate_context: "PCB origin aligned to fixture origin".to_string(),
                backing_board: "MDF spoilboard".to_string(),
            }],
            selected_fixture_id: Some("fixture-default".to_string()),
            job_profiles: vec![],
            selected_job_profile_id: None,
            machine_mru: vec![],
            focus_profile_name_editor: false,
            catalogs: built_in_catalogs(),
            tools,
            errors: vec![],
            generation_state: GenerationState::Idle,
            job_config: JobConfig {
                selected_operations: vec![ProductionOperation::DrillPth],
                side: Side::Top,
                rotation_mode: RotationMode::Auto,
                rotation_angle: 0,
                atc_strategy: AtcRackStrategy::Reuse,
                tab_count: 4,
                tab_width_mm: 3.0,
                tab_width_baseline_mm: 3.0,
                allow_routing_holes: true,
                drill_then_route: false,
                pilot_hole_fallback: true,
                cut_depth_strategy: CutDepthStrategy::Automatic,
                multi_pass_max_depth_mm: 1.0,
                outline_router_tool_id: None,
                board_thickness_mode: BoardThicknessMode::Automatic,
                board_thickness_preset_mm: 1.6,
                board_thickness_user_value: 1.6,
                z0_determination_mode: Z0DeterminationMode::ManualAdjustZ0,
                touch_probe_source: TouchProbeSource::ManualInstallation,
                touch_probe_atc_slot: 0,
                mouse_bites_enabled: false,
                mouse_bite_pitch_mm: 0.8,
                mouse_bite_drill_tool_id: None,
                board_orientation: BoardOrientation::Automatic,
                board_orientation_custom_degrees: 0.0,
            },
            gcode: sample_gcode(),
            save_filename: save_filename_override.unwrap_or_else(|| "output.nc".to_string()),
            gcode_modified: false,
            show_first_launch: true,
            rack_slots: BTreeMap::new(),
            board_layers: BoardLayers {
                holes: true,
                routes: true,
                paths: true,
                tabs: true,
            },
            board: board_snapshot,
        };

        state.seed_rack_slots(8);
        state
    }

    pub fn selected_machine(&self) -> Option<&MachineProfile> {
        self.selected_machine_id
            .as_ref()
            .and_then(|id| self.machines.iter().find(|m| &m.id == id))
    }

    pub fn selected_fixture(&self) -> Option<&FixtureProfile> {
        self.selected_fixture_id
            .as_ref()
            .and_then(|id| self.fixtures.iter().find(|fixture| &fixture.id == id))
    }

    pub fn selected_job_profile(&self) -> Option<&JobProfile> {
        self.selected_job_profile_id
            .as_ref()
            .and_then(|id| self.job_profiles.iter().find(|profile| &profile.id == id))
    }

    pub fn selected_machine_has_atc(&self) -> bool {
        self.selected_machine()
            .map(|m| m.atc_slot_count > 0)
            .unwrap_or(false)
    }

    fn make_machine_id(&self, base_name: &str) -> String {
        let base = slug(base_name);
        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                format!("profile-{}", base)
            } else {
                format!("profile-{}-{}", base, index)
            };
            if !self.machines.iter().any(|m| m.id == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn unique_machine_name(&self, base_name: &str, exclude_id: Option<&str>) -> String {
        let trimmed = base_name.trim();
        let base = if trimmed.is_empty() {
            "CNC profile".to_string()
        } else {
            trimmed.to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };

            let exists = self
                .machines
                .iter()
                .any(|m| Some(m.id.as_str()) != exclude_id && m.name == candidate);

            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn unique_copy_name(&self, source_name: &str) -> String {
        let base = format!("{} - copy", source_name.trim());
        let first = self.unique_machine_name(&base, None);
        if first == base {
            return first;
        }

        let mut index = 2usize;
        loop {
            let candidate = format!("{} - copy ({})", source_name.trim(), index);
            if !self.machines.iter().any(|m| m.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn select_machine_profile_by_id(&mut self, id: Option<String>) {
        self.selected_machine_id = id.clone();
        if let Some(id) = id {
            self.machine_mru.retain(|m| m != &id);
            self.machine_mru.insert(0, id);
        }
    }

    pub fn add_machine_profile(&mut self, mut profile: MachineProfile) {
        profile.name = self.unique_machine_name(&profile.name, None);
        profile.id = self.make_machine_id(&profile.name);
        let selected = profile.id.clone();
        self.machines.push(profile.clone());
        self.seed_rack_slots(profile.atc_slot_count);
        self.show_first_launch = false;
        self.select_machine_profile_by_id(Some(selected));

        if self.job_profiles.is_empty() {
            let fixture_id = self
                .selected_fixture_id
                .clone()
                .or_else(|| self.fixtures.first().map(|fixture| fixture.id.clone()));
            if let Some(fixture_id) = fixture_id {
                let job_profile = JobProfile {
                    id: "job-profile-default".to_string(),
                    name: "Default job profile".to_string(),
                    cnc_profile_id: profile.id.clone(),
                    fixture_profile_id: fixture_id,
                };
                self.selected_job_profile_id = Some(job_profile.id.clone());
                self.job_profiles.push(job_profile);
            }
        }
    }

    pub fn rename_selected_machine(&mut self, new_name: &str) -> Result<String, String> {
        let Some(selected) = self.selected_machine_id.clone() else {
            return Err("No CNC profile selected".to_string());
        };

        let unique = self.unique_machine_name(new_name, Some(selected.as_str()));
        if unique != new_name.trim() {
            return Err(format!("Profile name must be unique. Suggested: {}", unique));
        }

        if let Some(target) = self.machines.iter_mut().find(|m| m.id == selected) {
            target.name = unique;
            return Ok(target.name.clone());
        }

        Err("Selected CNC profile was not found".to_string())
    }

    pub fn add_demo_machine(&mut self) {
        let machine = MachineProfile {
            name: format!("Demo CNC profile {}", self.machines.len() + 1),
            fixture_plate_max_x: 300,
            fixture_plate_max_y: 200,
            spindle_min_rpm: 5000,
            spindle_max_rpm: 24000,
            atc_slot_count: 8,
            ..MachineProfile::default()
        };

        self.add_machine_profile(machine);
    }

    pub fn clone_selected_machine(&mut self) {
        let Some(current) = self.selected_machine().cloned() else {
            return;
        };

        let name = self.unique_copy_name(&current.name);
        let clone = MachineProfile {
            id: String::new(),
            name,
            ..current
        };

        self.add_machine_profile(clone);
        self.focus_profile_name_editor = true;
    }

    pub fn add_fixture_profile(&mut self, name: &str) {
        let base = if name.trim().is_empty() {
            "Fixture profile"
        } else {
            name.trim()
        };
        let mut idx = 1usize;
        let unique_name = loop {
            let candidate = if idx == 1 {
                base.to_string()
            } else {
                format!("{} ({})", base, idx)
            };
            if !self.fixtures.iter().any(|fixture| fixture.name == candidate) {
                break candidate;
            }
            idx += 1;
        };
        let fixture_id = format!("fixture-{}", slug(&unique_name));
        self.fixtures.push(FixtureProfile {
            id: fixture_id.clone(),
            name: unique_name,
            coordinate_context: "Fixture-defined board origin".to_string(),
            backing_board: "MDF spoilboard".to_string(),
        });
        self.selected_fixture_id = Some(fixture_id);
    }

    pub fn add_job_profile(&mut self, name: &str) {
        let Some(cnc_id) = self
            .selected_machine_id
            .clone()
            .or_else(|| self.machines.first().map(|machine| machine.id.clone()))
        else {
            return;
        };
        let Some(fixture_id) = self
            .selected_fixture_id
            .clone()
            .or_else(|| self.fixtures.first().map(|fixture| fixture.id.clone()))
        else {
            return;
        };

        let base = if name.trim().is_empty() {
            "Job profile"
        } else {
            name.trim()
        };
        let mut idx = 1usize;
        let unique_name = loop {
            let candidate = if idx == 1 {
                base.to_string()
            } else {
                format!("{} ({})", base, idx)
            };
            if !self.job_profiles.iter().any(|profile| profile.name == candidate) {
                break candidate;
            }
            idx += 1;
        };

        let id = format!("job-profile-{}", slug(&unique_name));
        self.job_profiles.push(JobProfile {
            id: id.clone(),
            name: unique_name,
            cnc_profile_id: cnc_id,
            fixture_profile_id: fixture_id,
        });
        self.selected_job_profile_id = Some(id);
    }

    pub fn impact_delete_cnc_profile(&self, cnc_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(cnc) = self.machines.iter().find(|machine| machine.id == cnc_id) {
            impact.primary_profiles.push(format!("CNC profile: {}", cnc.name));
        }

        let dependent_ids: BTreeSet<String> = self
            .job_profiles
            .iter()
            .filter(|profile| profile.cnc_profile_id == cnc_id)
            .map(|profile| profile.id.clone())
            .collect();

        for profile in self
            .job_profiles
            .iter()
            .filter(|profile| dependent_ids.contains(&profile.id))
        {
            impact
                .dependent_job_profiles
                .push(format!("Job profile: {}", profile.name));
        }

        if self
            .selected_job_profile_id
            .as_ref()
            .map(|id| dependent_ids.contains(id))
            .unwrap_or(false)
        {
            impact.deleted_live_jobs.push("Active job session".to_string());
        }

        impact
    }

    pub fn impact_delete_fixture_profile(&self, fixture_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(fixture) = self.fixtures.iter().find(|item| item.id == fixture_id) {
            impact
                .primary_profiles
                .push(format!("Fixture profile: {}", fixture.name));
        }

        let dependent_ids: BTreeSet<String> = self
            .job_profiles
            .iter()
            .filter(|profile| profile.fixture_profile_id == fixture_id)
            .map(|profile| profile.id.clone())
            .collect();

        for profile in self
            .job_profiles
            .iter()
            .filter(|profile| dependent_ids.contains(&profile.id))
        {
            impact
                .dependent_job_profiles
                .push(format!("Job profile: {}", profile.name));
        }

        if self
            .selected_job_profile_id
            .as_ref()
            .map(|id| dependent_ids.contains(id))
            .unwrap_or(false)
        {
            impact.deleted_live_jobs.push("Active job session".to_string());
        }

        impact
    }

    pub fn impact_delete_job_profile(&self, job_profile_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(profile) = self
            .job_profiles
            .iter()
            .find(|profile| profile.id == job_profile_id)
        {
            impact
                .primary_profiles
                .push(format!("Job profile: {}", profile.name));
        }
        if self
            .selected_job_profile_id
            .as_deref()
            .map(|id| id == job_profile_id)
            .unwrap_or(false)
        {
            impact.deleted_live_jobs.push("Active job session".to_string());
        }
        impact
    }

    pub fn delete_cnc_profile_with_cascade(&mut self, cnc_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_cnc_profile(cnc_id);

        self.machines.retain(|machine| machine.id != cnc_id);
        self.machine_mru.retain(|id| id != cnc_id);

        self.job_profiles
            .retain(|profile| profile.cnc_profile_id != cnc_id);

        if self
            .selected_job_profile_id
            .as_ref()
            .map(|id| !self.job_profiles.iter().any(|profile| &profile.id == id))
            .unwrap_or(false)
        {
            self.selected_job_profile_id = self.job_profiles.first().map(|profile| profile.id.clone());
        }

        let next_selected = self
            .machine_mru
            .iter()
            .find(|id| self.machines.iter().any(|machine| &machine.id == *id))
            .cloned()
            .or_else(|| self.machines.first().map(|machine| machine.id.clone()));

        self.select_machine_profile_by_id(next_selected);

        if self.machines.is_empty() {
            self.show_first_launch = true;
            self.selected_screen = Screen::CncProfiles;
        }

        impact
    }

    pub fn delete_fixture_profile_with_cascade(&mut self, fixture_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_fixture_profile(fixture_id);

        self.fixtures.retain(|fixture| fixture.id != fixture_id);
        self.job_profiles
            .retain(|profile| profile.fixture_profile_id != fixture_id);

        if self
            .selected_job_profile_id
            .as_ref()
            .map(|id| !self.job_profiles.iter().any(|profile| &profile.id == id))
            .unwrap_or(false)
        {
            self.selected_job_profile_id = self.job_profiles.first().map(|profile| profile.id.clone());
        }

        if self
            .selected_fixture_id
            .as_ref()
            .map(|id| !self.fixtures.iter().any(|fixture| &fixture.id == id))
            .unwrap_or(false)
        {
            self.selected_fixture_id = self.fixtures.first().map(|fixture| fixture.id.clone());
        }

        impact
    }

    pub fn delete_job_profile_with_cascade(&mut self, job_profile_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_job_profile(job_profile_id);
        self.job_profiles.retain(|profile| profile.id != job_profile_id);
        if self
            .selected_job_profile_id
            .as_ref()
            .map(|id| !self.job_profiles.iter().any(|profile| &profile.id == id))
            .unwrap_or(false)
        {
            self.selected_job_profile_id = self.job_profiles.first().map(|profile| profile.id.clone());
        }
        impact
    }

    #[allow(dead_code)]
    pub fn add_demo_tool(&mut self) {
        let idx = self.tools.len() + 1;
        self.tools.push(Tool {
            id: format!("tool-{idx}"),
            composite_name: format!("0.6mm End Mill {idx}"),
            name: String::new(),
            kind: "End Mill".to_string(),
            diameter: Length::from_mm(0.6),
            catalog_diameter: None,
            point_angle: Angle::from_degrees(180.0),
            catalog_point_angle: None,
            feed_rate: None,
            catalog_feed_rate: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: ToolStatus::InStock,
            preference: ToolPreference::Neutral,
            source_catalog: "Manual".to_string(),
            manufacturer: None,
            sku: None,
        });
    }

    fn next_tool_id(&self) -> String {
        let mut idx = self.tools.len() + 1;
        loop {
            let candidate = format!("tool-{idx}");
            if !self.tools.iter().any(|t| t.id == candidate) {
                return candidate;
            }
            idx += 1;
        }
    }

    fn unique_tool_clone_name(&self, source: &Tool) -> String {
        let base = if source.name.trim().is_empty() {
            "Copy".to_string()
        } else {
            format!("{} copy", source.name.trim())
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} {}", base, index)
            };
            let display_name = format!("{} - {}", source.composite_name.trim(), candidate);
            if !self
                .tools
                .iter()
                .any(|tool| tool.display_name().eq_ignore_ascii_case(&display_name))
            {
                return candidate;
            }
            index += 1;
        }
    }

    fn unique_catalog_name(&self, base_name: &str) -> String {
        let base = if base_name.trim().is_empty() {
            "Catalog".to_string()
        } else {
            base_name.trim().to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };
            if !self.catalogs.iter().any(|c| c.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    fn unique_catalog_key(&self, base: &str) -> String {
        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.to_string()
            } else {
                format!("{}-{}", base, index)
            };
            if !self.catalogs.iter().any(|c| c.key == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn import_catalog_text(&mut self, stem: &str, yaml_text: &str) -> Result<String, String> {
        let catalog = parse_catalog_with_backfill(yaml_text, stem)
            .map_err(|_| "Catalog import failed: invalid YAML or schema".to_string())?;
        let unique_name = self.unique_catalog_name(&catalog.name);
        let key_base = format!("import-{}", slug(stem));
        let unique_key = self.unique_catalog_key(&key_base);
        let stock_catalog = catalog_to_stock_catalog(&unique_key, &unique_name, &catalog, false);
        self.catalogs.push(stock_catalog);
        Ok(unique_name)
    }

    pub fn remove_catalog(&mut self, catalog_key: &str) -> Result<(), String> {
        let Some(entry) = self.catalogs.iter().find(|c| c.key == catalog_key).cloned() else {
            return Err("Catalog not found".to_string());
        };

        if entry.built_in {
            return Err("Built-in catalogs cannot be deleted".to_string());
        }

        self.catalogs.retain(|c| c.key != catalog_key);
        Ok(())
    }

    pub fn add_tools_from_catalog_selection(&mut self, selected_tool_keys: &[String]) -> usize {
        if selected_tool_keys.is_empty() {
            return 0;
        }

        let mut added = 0usize;
        for catalog in &self.catalogs {
            for section in &catalog.sections {
                for tool in &section.tools {
                    if !selected_tool_keys.iter().any(|k| k == &tool.key) {
                        continue;
                    }

                    let has_same_sku = tool
                        .sku
                        .as_ref()
                        .map(|sku| !sku.trim().is_empty())
                        .unwrap_or(false)
                        && self
                            .tools
                            .iter()
                            .any(|existing| {
                                existing
                                    .sku
                                    .as_ref()
                                    .map(|x| x == tool.sku.as_ref().unwrap())
                                    .unwrap_or(false)
                            });
                    let has_same_identity = self
                        .tools
                        .iter()
                        .any(|existing| {
                            existing.composite_name == tool.display_name
                                && existing.kind == tool.kind
                                && (existing.diameter.as_mm() - tool.diameter.as_mm()).abs() < 0.0001
                        });
                    if has_same_sku || has_same_identity {
                        continue;
                    }

                    self.tools.push(Tool {
                        id: self.next_tool_id(),
                        composite_name: tool.display_name.clone(),
                        name: String::new(),
                        kind: tool.kind.clone(),
                        diameter: tool.diameter,
                        catalog_diameter: Some(tool.diameter),
                        point_angle: tool.point_angle,
                        catalog_point_angle: Some(tool.point_angle),
                        feed_rate: tool.feed_rate,
                        catalog_feed_rate: tool.feed_rate,
                        spindle_speed: tool.spindle_speed,
                        catalog_spindle_speed: tool.spindle_speed,
                        status: ToolStatus::InStock,
                        preference: ToolPreference::Neutral,
                        source_catalog: format!("{} / {}", catalog.name, section.name),
                        manufacturer: Some(format!("{} / {}", catalog.name, section.name)),
                        sku: tool.sku.clone(),
                    });
                    added += 1;
                }
            }
        }

        added
    }

    pub fn clone_tool(&mut self, tool_id: &str) -> Option<String> {
        let source = self.tools.iter().find(|tool| tool.id == tool_id).cloned()?;
        let new_id = self.next_tool_id();
        let clone = Tool {
            id: new_id.clone(),
            name: self.unique_tool_clone_name(&source),
            ..source
        };
        self.tools.push(clone);
        Some(new_id)
    }

    pub fn remove_tools(&mut self, tool_ids: &[String]) -> usize {
        if tool_ids.is_empty() {
            return 0;
        }

        let to_remove: BTreeSet<&str> = tool_ids.iter().map(|tool_id| tool_id.as_str()).collect();
        let before = self.tools.len();

        self.tools.retain(|tool| !to_remove.contains(tool.id.as_str()));

        for slot in self.rack_slots.values_mut() {
            if slot
                .tool_id
                .as_deref()
                .map(|tool_id| to_remove.contains(tool_id))
                .unwrap_or(false)
            {
                slot.tool_id = None;
            }
        }

        if self
            .job_config
            .outline_router_tool_id
            .as_deref()
            .map(|tool_id| to_remove.contains(tool_id))
            .unwrap_or(false)
        {
            self.job_config.outline_router_tool_id = None;
        }

        if self
            .job_config
            .mouse_bite_drill_tool_id
            .as_deref()
            .map(|tool_id| to_remove.contains(tool_id))
            .unwrap_or(false)
        {
            self.job_config.mouse_bite_drill_tool_id = None;
        }

        before.saturating_sub(self.tools.len())
    }

    pub fn select_screen(&mut self, screen: Screen) {
        self.selected_screen = screen;
    }

    pub fn toggle_operation(&mut self, op: ProductionOperation) {
        if let Some(index) = self
            .job_config
            .selected_operations
            .iter()
            .position(|x| *x == op)
        {
            self.job_config.selected_operations.remove(index);
        } else {
            self.job_config.selected_operations.push(op);
        }
        self.gcode_modified = false;
    }

    #[allow(dead_code)]
    pub fn set_rotation_angle(&mut self, angle: i32) {
        self.job_config.rotation_angle = angle;
        self.gcode_modified = false;
    }

    pub fn seed_rack_slots(&mut self, slot_count: u8) {
        for slot in 1..=slot_count {
            self.rack_slots.entry(slot).or_insert(RackSlot {
                tool_id: None,
                locked: false,
                disabled: false,
            });
        }
    }
}

fn load_persisted_unit_system() -> UnitSystem {
    let Some(state) = persistence_state() else {
        return UnitSystem::Metric;
    };

    match state
        .global_settings
        .get("units")
        .and_then(|units| units.get("system"))
        .and_then(|system| system.as_str())
    {
        Some("mil") => UnitSystem::Mil,
        Some("imperial") => UnitSystem::Imperial,
        _ => UnitSystem::Metric,
    }
}

fn load_persisted_theme() -> Theme {
    let Some(state) = persistence_state() else {
        return Theme::Dark;
    };

    let theme_mode = state
        .global_settings
        .get("theme")
        .and_then(|theme| theme.get("mode"))
        .and_then(|mode| mode.as_str())
        .unwrap_or("dark");

    Theme::from_str(theme_mode)
}

pub fn sample_gcode() -> String {
    "; KiCad CNC Generator - GCode Output\n\
; Generated from Dioxus UI\n\
G21\n\
G90\n\
M3 S12000\n\
G0 Z5.0\n\
G0 X20.0 Y20.0\n\
G1 Z-1.6 F200\n\
G0 Z5.0\n\
G0 X180.0 Y130.0\n\
G1 Z-1.6 F200\n\
G0 Z5.0\n\
M5\n\
M30\n"
        .to_string()
}

fn catalog_to_stock_catalog(
    key: &str,
    display_name: &str,
    catalog: &Catalog,
    built_in: bool,
) -> CatalogStockCatalog {
    let mut sections = Vec::new();

    for (section_idx, section) in catalog.sections.iter().enumerate() {
        let section_key = format!("{}::s{}", key, section_idx);
        let mut tools = Vec::new();

        for (tool_idx, tool) in section.tools.iter().enumerate() {
            let feed_rate = tool.table_feed.or(tool.z_feed);
            let kind = match tool.tool_type {
                ToolType::Drillbit => "Drill",
                ToolType::Routerbit => "Router",
            }
            .to_string();

            let display_tool_name = if tool.sku_name.trim().is_empty() {
                format!("{} {}", kind, tool.diameter.unit_display(UserUnitSystem::Metric).user)
            } else {
                tool.sku_name.clone()
            };

            tools.push(CatalogStockTool {
                key: format!("{}::t{}", section_key, tool_idx),
                catalog_name: display_name.to_string(),
                section_name: section.name.clone(),
                display_name: display_tool_name,
                kind,
                diameter: tool.diameter,
                point_angle: tool.point_angle,
                feed_rate,
                spindle_speed: tool.spindle_rpm,
                sku: Some(tool.sku_name.clone()),
            });
        }

        sections.push(CatalogStockSection {
            key: section_key,
            name: section.name.clone(),
            tools,
        });
    }

    CatalogStockCatalog {
        key: key.to_string(),
        name: display_name.to_string(),
        built_in,
        sections,
    }
}

fn built_in_catalogs() -> Vec<CatalogStockCatalog> {
    let sources = [
        ("kyocera", include_str!("../../resources/catalogs/kyocera.yaml")),
        ("unionfab", include_str!("../../resources/catalogs/unionfab.yaml")),
        ("generic", include_str!("../../resources/catalogs/generic.yaml")),
    ];

    let mut out = Vec::new();
    for (stem, text) in sources {
        if let Ok(catalog) = parse_catalog_with_backfill(text, stem) {
            let key = format!("builtin-{}", slug(stem));
            out.push(catalog_to_stock_catalog(&key, &catalog.name, &catalog, true));
        }
    }
    out
}
