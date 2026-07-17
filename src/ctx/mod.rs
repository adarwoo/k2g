use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;
use rhai::{Array, Dynamic, Map};

use crate::board::BoardSnapshot;
use crate::config::{
    ConfigError,
    backfill_catalog_fields, ensure_default_files, load_all_configs, normalize_catalog_fields,
    write_embedded_schemas, PersistenceState, load_all_configs_best_effort,
    save_cnc_profiles, save_fixture_profiles, save_global_settings, save_processing_profiles,
    save_processing_and_toolset_profiles_session, save_stock, save_toolset_profiles,
};
use crate::config::yaml_service::parse_yaml_with_schema;
use crate::domain::catalog::{catalog_dir, default_catalogs, Catalog, CatalogManager};
use crate::domain::state::RackSlot;
use crate::domain::stock::{stock_value_from_tools, tools_from_stock_value};
use crate::ui::model::{
    CascadeDeleteImpact, CatalogStockCatalog, CatalogStockSection, CatalogStockTool,
    CutDepthStrategy, FixtureProfile, GenerationState, JobCenterView, JobConfig, JobProfile,
    MachineProfile, PersistRealm, ProductionOperation, Screen, Side, Theme, Tool,
    ToolPreference, ToolStatus, ToolsetGenerationPolicy, ToolsetProfile, UiLaunchData,
    UnitSystem,
};
use crate::units::{Angle, FeedRate, Length, RotationalSpeed};
use crate::user_path::{
    AppDirs,
    ensure_app_dirs,
};
use crate::stitching::stitch_edge_shapes;
use serde_json::{json, Value};
use uuid::Uuid;

pub const STATUS_KEY_REGENERATION: &str = "regeneration.status";
pub const STATUS_KEY_KICAD: &str = "kicad.status";
pub const STATUS_KEY_PROJECT_HAS_BOARD: &str = "project.has_board";
pub const STATUS_KEY_PROJECT_SELECTED_PROCESS: &str = "project.selected_process_profile";

#[derive(Clone, Copy)]
pub enum UiCommand {
    SetUnitSystem(UnitSystem),
    ToggleTheme,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct CtxIssue {
    pub id: String,
    pub domain: String,
    pub is_error: bool,
    pub message: String,
    pub details: Option<String>,
    pub created_ms: u64,
}

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

/// Canonical runtime application state.
#[derive(Clone)]
pub struct AppState {
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

include!("app_state_impl.rs");

#[allow(dead_code)]
#[derive(Clone)]
pub struct StitchedBoardData {
    pub contour_count: usize,
    pub error_count: usize,
    pub errors: Vec<String>,
}

/// Canonical application state owned by context.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppCtx {
    pub app: AppState,
    pub cli_args: Vec<String>,
    pub stitched_board_data: Option<StitchedBoardData>,
    pub kicad_status: String,
    pub issues: Vec<CtxIssue>,
    pub status: BTreeMap<String, String>,
    pub catalogs_loaded: bool,
}

include!("runtime_impl.rs");

include!("catalogs_impl.rs");

static GLOBAL_CTX: OnceLock<RwLock<AppCtx>> = OnceLock::new();
static PERSISTENCE_STATE: OnceLock<PersistenceState> = OnceLock::new();

pub fn initialize_ctx(boot: UiLaunchData) {
    if let Err(e) = write_embedded_schemas() {
        warn!("Could not refresh embedded schemas in user data dir: {e}");
    }

    if let Ok(app_dirs) = ensure_app_dirs() {
        let persistence_state = load_all_configs(&app_dirs, &app_dirs.schemas)
            .unwrap_or_else(|err| {
                warn!(
                    "Could not load full persistence state ({}); falling back to best-effort load.",
                    err
                );
                fallback_persistence_state(&app_dirs)
            });
        let _ = PERSISTENCE_STATE.set(persistence_state);
    }

    let _ = GLOBAL_CTX.set(RwLock::new(AppCtx::from_launch(&boot)));
}

#[allow(dead_code)]
pub fn persistence_state() -> Option<&'static PersistenceState> {
    PERSISTENCE_STATE.get()
}

pub fn ctx_snapshot() -> AppCtx {
    with_ctx(Clone::clone)
}

pub fn with_ctx<R>(f: impl FnOnce(&AppCtx) -> R) -> R {
    let lock = GLOBAL_CTX
        .get()
        .expect("Global ctx must be initialized before use");
    let guard = lock
        .read()
        .expect("Global ctx read lock should not be poisoned");
    f(&guard)
}

pub fn with_ctx_mut<R>(f: impl FnOnce(&mut AppCtx) -> R) -> R {
    let lock = GLOBAL_CTX
        .get()
        .expect("Global ctx must be initialized before use");
    let mut guard = lock
        .write()
        .expect("Global ctx write lock should not be poisoned");
    f(&mut guard)
}

pub fn apply_ui_command(command: UiCommand) {
    with_ctx_mut(|ctx| {
        match command {
            UiCommand::SetUnitSystem(unit_system) => {
                ctx.app.unit_system = unit_system;
            }
            UiCommand::ToggleTheme => {
                ctx.app.theme = if ctx.app.theme == Theme::Dark {
                    Theme::Light
                } else {
                    Theme::Dark
                };
            }
        }

        let updated_ui = ctx.app.clone();
        updated_ui.persist_realms(&[PersistRealm::GlobalSettings]);
        ctx.sync_from_app_state(&updated_ui);
    });
}

#[allow(dead_code)]
pub fn ensure_catalogs_loaded() {
    with_ctx_mut(|ctx| ctx.ensure_catalogs_loaded());
}

#[allow(dead_code)]
pub fn refresh_catalogs() {
    with_ctx_mut(|ctx| ctx.refresh_catalogs());
}

#[allow(dead_code)]
pub fn rhai_read_only_ctx() -> Map {
    with_ctx(|ctx| ctx.as_rhai_ctx())
}

