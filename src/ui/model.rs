use std::collections::BTreeMap;

use crate::catalog::init::catalog_dir;
use crate::catalog::types::{Catalog, FeedUnit, LinearUnit, ToolType};
use crate::catalog::CatalogManager;

#[derive(Clone, PartialEq)]
pub struct UiLaunchData {
    pub env_vars: Vec<(String, String)>,
    pub env_summary: String,
    pub cli_args: Vec<String>,
    pub kicad_status: String,
    pub save_filename_override: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Setup,
    Stock,
    Job,
    BoardView,
    Program,
    Rack,
}

impl Screen {
    pub fn label(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Stock => "Stock",
            Self::Job => "Job",
            Self::BoardView => "Board View",
            Self::Program => "Program",
            Self::Rack => "Rack",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Stock => "stock",
            Self::Job => "job",
            Self::BoardView => "board-view",
            Self::Program => "program",
            Self::Rack => "rack",
        }
    }

    pub fn visible(has_atc: bool) -> Vec<Self> {
        let mut screens = vec![
            Self::Setup,
            Self::Stock,
            Self::Job,
            Self::BoardView,
            Self::Program,
        ];
        if has_atc {
            screens.push(Self::Rack);
        }
        screens
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnitSystem {
    Metric,
    Imperial,
}

impl UnitSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metric => "metric",
            Self::Imperial => "imperial",
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
}

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
    pub fixture_plate_max_x: u32,
    pub fixture_plate_max_y: u32,
    pub spindle_min_rpm: u32,
    pub spindle_max_rpm: u32,
    pub atc_slot_count: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    InStock,
    InRack,
    OutOfStock,
    New,
    NotPreferred,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::InStock => "In Stock",
            Self::InRack => "In Rack",
            Self::OutOfStock => "Out Of Stock",
            Self::New => "New",
            Self::NotPreferred => "Not Preferred",
        }
    }

    pub fn class_name(self) -> &'static str {
        match self {
            Self::InStock => "status-in-stock",
            Self::InRack => "status-in-rack",
            Self::OutOfStock => "status-out-of-stock",
            Self::New => "status-new",
            Self::NotPreferred => "status-not-preferred",
        }
    }
}

#[derive(Clone)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub diameter_mm: f32,
    pub feed_rate_mm_min: Option<f32>,
    pub spindle_rpm: Option<u32>,
    pub status: ToolStatus,
    pub operation_count: u32,
    pub manufacturer: Option<String>,
    pub sku: Option<String>,
    /// True when the tool was originally specified in inches (e.g. from an
    /// imperial catalog section).  Used to decide whether to show a unit
    /// conversion hint in the stock list.
    pub native_is_inch: bool,
}

#[derive(Clone)]
pub struct CatalogStockTool {
    pub key: String,
    pub catalog_name: String,
    pub section_name: String,
    pub display_name: String,
    pub kind: String,
    pub diameter_mm: f32,
    pub feed_rate_mm_min: Option<f32>,
    pub spindle_rpm: Option<u32>,
    pub sku: Option<String>,
    pub native_is_inch: bool,
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
    pub sections: Vec<CatalogStockSection>,
}

pub fn load_stock_catalog_index() -> Vec<CatalogStockCatalog> {
    // Primary source: user catalog directory.  Files are parsed and validated
    // by CatalogManager; only valid catalogs are included.
    let mut source_catalogs: Vec<(String, Catalog)> = Vec::new();

    if let (Ok(mut manager), Ok(dir)) = (CatalogManager::new(), catalog_dir()) {
        let _ = manager.load_dir(&dir);
        source_catalogs = manager
            .catalogs()
            .map(|(stem, catalog)| (stem.to_string(), catalog.clone()))
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
            let normalized = text.trim_start_matches('\u{feff}');
            if let Ok(catalog) = serde_yaml::from_str::<Catalog>(normalized) {
                source_catalogs.push((stem, catalog));
            }
        }
    }

    let mut out = Vec::new();

    for (stem, catalog) in source_catalogs {
        let catalog_key = slug(&stem);
        let mut sections = Vec::new();

        for (section_idx, section) in catalog.sections.iter().enumerate() {
            let section_key = format!("{}::s{}", catalog_key, section_idx);
            let mut tools = Vec::new();

            for (tool_idx, tool) in section.tools.iter().enumerate() {
                let diameter_unit = tool.diameter_unit.unwrap_or(section.default_diameter_unit);
                let diameter_mm = if diameter_unit == LinearUnit::In {
                    tool.diameter * 25.4
                } else {
                    tool.diameter
                } as f32;

                let feed_rate_mm_min = tool
                    .table_feed
                    .or(tool.z_feed)
                    .map(|feed| match section.default_feed_unit {
                        FeedUnit::MmMin => feed,
                        FeedUnit::Ipm => feed * 25.4,
                    } as f32);

                let kind = match tool.tool_type {
                    ToolType::Drillbit => "Drill",
                    ToolType::Routerbit => "Router",
                }
                .to_string();

                let display_name = if tool.sku_name.trim().is_empty() {
                    format!("{} {:.3}mm", kind, diameter_mm)
                } else {
                    tool.sku_name.clone()
                };

                tools.push(CatalogStockTool {
                    key: format!("{}::t{}", section_key, tool_idx),
                    catalog_name: catalog.name.clone(),
                    section_name: section.name.clone(),
                    display_name,
                    kind,
                    diameter_mm,
                    feed_rate_mm_min,
                    spindle_rpm: tool.spindle_rpm,
                    sku: Some(tool.sku_name.clone()),
                    native_is_inch: diameter_unit == LinearUnit::In,
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
    Front,
    Back,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RotationMode {
    Auto,
    Manual,
}

impl RotationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AtcRackStrategy {
    Reuse,
    Overwrite,
}

impl AtcRackStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reuse => "reuse",
            Self::Overwrite => "overwrite",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProductionOperation {
    DrillLocatingPins,
    DrillPth,
    DrillNpth,
    RouteBoard,
    MillBoard,
}

impl ProductionOperation {
    pub fn all() -> [Self; 5] {
        [
            Self::DrillLocatingPins,
            Self::DrillPth,
            Self::DrillNpth,
            Self::RouteBoard,
            Self::MillBoard,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::DrillLocatingPins => "Drill Locating Pins",
            Self::DrillPth => "Drill PTH",
            Self::DrillNpth => "Drill NPTH",
            Self::RouteBoard => "Route Board",
            Self::MillBoard => "Mill Board",
        }
    }
}

#[derive(Clone)]
pub struct JobConfig {
    pub selected_operations: Vec<ProductionOperation>,
    pub side: Side,
    pub rotation_mode: RotationMode,
    pub rotation_angle: i32,
    pub atc_rack_strategy: AtcRackStrategy,
    pub tab_count: u8,
    pub tab_width_mm: f32,
    pub allow_routing_holes: bool,
    pub drill_then_route: bool,
    pub pilot_hole_fallback: bool,
}

#[derive(Clone)]
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
    pub unit_system: UnitSystem,
    pub theme: Theme,
    pub machines: Vec<MachineProfile>,
    pub selected_machine_id: Option<String>,
    pub tools: Vec<Tool>,
    pub errors: Vec<AppError>,
    pub generation_state: GenerationState,
    pub job_config: JobConfig,
    pub gcode: String,
    pub save_filename: String,
    pub gcode_modified: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    pub board_layers: BoardLayers,
}

impl UiState {
    pub fn new(save_filename_override: Option<String>) -> Self {
        let tools = vec![
            Tool {
                id: "tool-1".to_string(),
                name: "0.8mm Carbide Drill".to_string(),
                kind: "Drill".to_string(),
                diameter_mm: 0.8,
                feed_rate_mm_min: Some(120.0),
                spindle_rpm: Some(18_000),
                status: ToolStatus::InStock,
                operation_count: 2,
                manufacturer: Some("CNCLab".to_string()),
                sku: Some("DRL-08".to_string()),
                native_is_inch: false,
            },
            Tool {
                id: "tool-2".to_string(),
                name: "1.0mm End Mill".to_string(),
                kind: "End Mill".to_string(),
                diameter_mm: 1.0,
                feed_rate_mm_min: Some(280.0),
                spindle_rpm: Some(16_000),
                status: ToolStatus::InRack,
                operation_count: 3,
                manufacturer: Some("CNCLab".to_string()),
                sku: Some("EM-10".to_string()),
                native_is_inch: false,
            },
            Tool {
                id: "tool-3".to_string(),
                name: "30deg V-Bit".to_string(),
                kind: "V-Bit".to_string(),
                diameter_mm: 0.2,
                feed_rate_mm_min: None,
                spindle_rpm: None,
                status: ToolStatus::New,
                operation_count: 1,
                manufacturer: None,
                sku: None,
                native_is_inch: false,
            },
        ];

        let mut state = Self {
            selected_screen: Screen::Job,
            unit_system: UnitSystem::Metric,
            theme: Theme::Dark,
            machines: vec![],
            selected_machine_id: None,
            tools,
            errors: vec![],
            generation_state: GenerationState::Idle,
            job_config: JobConfig {
                selected_operations: vec![
                    ProductionOperation::DrillPth,
                    ProductionOperation::RouteBoard,
                ],
                side: Side::Front,
                rotation_mode: RotationMode::Auto,
                rotation_angle: 0,
                atc_rack_strategy: AtcRackStrategy::Reuse,
                tab_count: 4,
                tab_width_mm: 3.0,
                allow_routing_holes: true,
                drill_then_route: false,
                pilot_hole_fallback: true,
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
        };

        state.seed_rack_slots(8);
        state
    }

    pub fn selected_machine(&self) -> Option<&MachineProfile> {
        self.selected_machine_id
            .as_ref()
            .and_then(|id| self.machines.iter().find(|m| &m.id == id))
    }

    pub fn selected_machine_has_atc(&self) -> bool {
        self.selected_machine()
            .map(|m| m.atc_slot_count > 0)
            .unwrap_or(false)
    }

    pub fn add_demo_machine(&mut self) {
        let id = format!("machine-{}", self.machines.len() + 1);
        let machine = MachineProfile {
            id: id.clone(),
            name: format!("Demo Machine {}", self.machines.len() + 1),
            fixture_plate_max_x: 300,
            fixture_plate_max_y: 200,
            spindle_min_rpm: 5000,
            spindle_max_rpm: 24000,
            atc_slot_count: 8,
        };

        self.machines.push(machine);
        self.selected_machine_id = Some(id);
        self.show_first_launch = false;
        self.seed_rack_slots(8);
    }

    pub fn clone_selected_machine(&mut self) {
        let Some(current) = self.selected_machine().cloned() else {
            return;
        };

        let id = format!("machine-{}", self.machines.len() + 1);
        let clone = MachineProfile {
            id: id.clone(),
            name: format!("Copy of {}", current.name),
            fixture_plate_max_x: current.fixture_plate_max_x,
            fixture_plate_max_y: current.fixture_plate_max_y,
            spindle_min_rpm: current.spindle_min_rpm,
            spindle_max_rpm: current.spindle_max_rpm,
            atc_slot_count: current.atc_slot_count,
        };

        self.machines.push(clone);
        self.selected_machine_id = Some(id);
    }

    pub fn remove_selected_machine(&mut self) {
        let Some(selected) = self.selected_machine_id.clone() else {
            return;
        };

        self.machines.retain(|m| m.id != selected);
        self.selected_machine_id = self.machines.first().map(|m| m.id.clone());

        if self.machines.is_empty() {
            self.show_first_launch = true;
            self.selected_screen = Screen::Setup;
        }
    }

    pub fn add_demo_tool(&mut self) {
        let idx = self.tools.len() + 1;
        self.tools.push(Tool {
            id: format!("tool-{idx}"),
            name: format!("Manual Tool {idx}"),
            kind: "End Mill".to_string(),
            diameter_mm: 0.6,
            feed_rate_mm_min: None,
            spindle_rpm: None,
            status: ToolStatus::InStock,
            operation_count: 0,
            manufacturer: None,
            sku: None,
            native_is_inch: false,
        });
    }

    pub fn select_screen(&mut self, screen: Screen) {
        if screen == Screen::Rack && !self.selected_machine_has_atc() {
            self.selected_screen = Screen::Setup;
            return;
        }
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
