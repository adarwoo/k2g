use std::collections::BTreeMap;

use crate::board::BoardSnapshot;
pub use crate::domain::state::RackSlot;

use super::job::JobConfig;
use super::profiles::{FixtureProfile, JobProfile, MachineProfile, ToolsetProfile};
use super::stock::{CatalogStockCatalog, Tool};
use super::ui_shell::{GenerationState, JobCenterView, Screen, Theme, UnitSystem};

/// Runtime diagnostic entry shown in UI.
#[allow(dead_code)]
#[derive(Clone, PartialEq)]
pub struct AppError {
    pub id: String,
    pub domain: String,
    pub is_error: bool,
    pub message: String,
    pub details: Option<String>,
}

/// Runtime event entry shown in UI notifications.
#[derive(Clone, PartialEq)]
pub struct AppEvent {
    pub id: String,
    pub message: String,
    pub created_ms: u64,
}

/// Visible board overlay layers in the board view.
#[allow(dead_code)]
#[derive(Clone)]
pub struct BoardLayers {
    pub holes: bool,
    pub routes: bool,
    pub paths: bool,
    pub tabs: bool,
}

/// Main UI aggregate state. All persistable domains are nested here.
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
    pub process_profiles: Vec<JobProfile>,
    pub selected_process_profile_id: Option<String>,
    pub last_edited_process_profile_id: Option<String>,
    pub toolsets: Vec<ToolsetProfile>,
    pub selected_toolset_id: Option<String>,
    pub machine_mru: Vec<String>,
    pub focus_profile_name_editor: bool,
    pub catalogs: Vec<CatalogStockCatalog>,
    pub tools: Vec<Tool>,
    pub errors: Vec<AppError>,
    pub events: Vec<AppEvent>,
    pub generation_state: GenerationState,
    pub project_config: JobConfig,
    pub gcode: String,
    pub save_filename: String,
    pub gcode_modified: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    #[allow(dead_code)]
    pub board_layers: BoardLayers,
    pub board: Option<BoardSnapshot>,
}
