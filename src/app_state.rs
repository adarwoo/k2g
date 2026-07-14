use std::collections::BTreeMap;

use crate::board::BoardSnapshot;
use crate::domain::job::JobConfig;
use crate::domain::profiles::{FixtureProfile, JobProfile, MachineProfile, ToolsetProfile};
use crate::domain::state::RackSlot;
use crate::domain::stock::Tool;
use crate::ui::model::{CatalogStockCatalog, GenerationState};

/// Canonical application state owned by ctx.
///
/// During migration this coexists with UiState so screens keep working.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    pub machines: Vec<MachineProfile>,
    pub selected_machine_id: Option<String>,
    pub fixtures: Vec<FixtureProfile>,
    pub selected_fixture_id: Option<String>,
    pub process_profiles: Vec<JobProfile>,
    pub selected_process_profile_id: Option<String>,
    pub last_edited_process_profile_id: Option<String>,
    pub toolsets: Vec<ToolsetProfile>,
    pub selected_toolset_id: Option<String>,
    pub catalogs: Vec<CatalogStockCatalog>,
    pub tools: Vec<Tool>,
    pub generation_state: GenerationState,
    pub project_config: JobConfig,
    pub gcode: String,
    pub gcode_modified: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    pub board: Option<BoardSnapshot>,
}
