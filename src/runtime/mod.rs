use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;
use rhai::{Array, Dynamic, Map};

use pcb::BoardSnapshot;
use crate::catalog_io::{
    backfill_catalog_fields, ensure_default_files, normalize_catalog_fields,
};
use crate::catalog_io::yaml_service::parse_yaml_with_schema;
use crate::data::model::catalog::{catalog_dir, default_catalogs, Catalog, CatalogManager};
use crate::data::model::state::RackSlot;
use crate::data::model::stock::{stock_value_from_tools, tools_from_stock_value};
use crate::data::model::{
    CascadeDeleteImpact, CatalogStockCatalog, CatalogStockSection, CatalogStockTool,
    CutDepthStrategy, FixtureProfile, GenerationState, JobCenterView, JobConfig, JobProfile,
    MachineProfile, PersistRealm, ProductionOperation, Screen, Side, Theme, Tool,
    ToolPreference, ToolStatus, ToolsetGenerationPolicy, ToolsetProfile, UiLaunchData,
    UnitSystem,
};
use units::{Angle, Length};
use crate::paths::{
    AppDirs,
    ensure_app_dirs,
};
use pcb::stitch_edge_shapes;
use serde_json::{json, Value};
use uuid::Uuid;

pub const STATUS_KEY_REGENERATION: &str = "regeneration.status";
pub const STATUS_KEY_KICAD: &str = "kicad.status";
pub const STATUS_KEY_PROJECT_HAS_BOARD: &str = "project.has_board";
pub const STATUS_KEY_PROJECT_SELECTED_PROCESS: &str = "project.selected_process_profile";
pub const STATUS_KEY_GENERATION_READINESS: &str = "generation.readiness_gate";
pub const STATUS_KEY_GENERATION_NOGO_REASONS: &str = "generation.nogo_reasons";
pub const STATUS_KEY_GENERATION_LAST_TRIGGER: &str = "generation.last_trigger_cause";
pub const STATUS_KEY_GENERATION_MODIFIED_UUIDS: &str = "generation.modified_uuids";

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
    pub owner_tag: Option<String>,
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
    pub owner_tag: Option<String>,
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
    pub catalogs: Vec<CatalogStockCatalog>,
    pub tools: Vec<Tool>,
    pub errors: Vec<AppError>,
    pub events: Vec<AppEvent>,
    pub generation_state: GenerationState,
    pub project_config: JobConfig,
    pub gcode: String,
    pub save_filename: String,
    pub gcode_modified: bool,
    pub suppress_persistence: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    #[allow(dead_code)]
    pub board_layers: BoardLayers,
    pub board: Option<BoardSnapshot>,
}

include!("state.rs");

#[allow(dead_code)]
#[derive(Clone)]
pub struct StitchedBoardData {
    pub contour_count: usize,
    pub error_count: usize,
    pub errors: Vec<String>,
}

/// Resolved references used by the active job.
#[allow(dead_code)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct JobReferences {
    pub process_profile_id: Option<String>,
    pub cnc_profile_id: Option<String>,
    pub fixture_profile_id: Option<String>,
    pub toolset_profile_id: Option<String>,
    pub referenced_tool_ids: BTreeSet<String>,
}

/// Canonical application state owned by context.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppCtx {
    pub app: AppState,
    pub cli_args: Vec<String>,
    pub stitched_board_data: Option<StitchedBoardData>,
    pub job_references: JobReferences,
    pub kicad_status: String,
    pub issues: Vec<CtxIssue>,
    pub status: BTreeMap<String, String>,
    pub catalogs_loaded: bool,
}

include!("orchestration.rs");

include!("catalogs.rs");

static GLOBAL_CTX: OnceLock<RwLock<AppCtx>> = OnceLock::new();
static PERSISTENCE_STATE: OnceLock<PersistenceState> = OnceLock::new();

/// The subset of persisted state the launch-time hydrate consumes. Formerly built
/// by the legacy `load_all_configs` loader; it is now sourced from
/// [`crate::data::AppData`] (the single reader/writer of every realm), which reads
/// the same on-disk files. This is just the shape [`AppState::hydrate_from_persistence`]
/// expects — the loading and validation now live in the datastore.
struct PersistenceState {
    global_settings: Value,
    stock: Value,
    cnc_profiles: BTreeMap<String, Value>,
    fixture_profiles: BTreeMap<String, Value>,
    processing_profiles: BTreeMap<String, Value>,
    toolset_profiles: BTreeMap<String, Value>,
    selected_process_profile_id: Option<String>,
    last_edited_process_profile_id: Option<String>,
    selected_cnc_profile_id: Option<String>,
    selected_fixture_profile_id: Option<String>,
    selected_toolset_profile_id: Option<String>,
}

/// Snapshots the hydrate state out of the live [`AppData`] store. Returns `None`
/// when the store is not ready (e.g. a headless test context), in which case the
/// hydrate falls back to in-memory defaults.
fn persistence_state_from_appdata() -> Option<PersistenceState> {
    if !crate::data::appdata_ready() {
        return None;
    }
    Some(crate::data::with_appdata(|data| {
        let global_settings = data
            .settings()
            .map(|doc| doc.to_value())
            .unwrap_or_else(default_global_settings);
        let stock = data
            .stock()
            .map(|doc| doc.to_value())
            .unwrap_or_else(|| json!({ "tools": [] }));

        // Each profile realm as an id→value map (hydrate iterates `.values()`; the
        // key is only for uniqueness, so the document id serves).
        let collect = |profile| {
            data.list(profile)
                .into_iter()
                .map(|(id, doc)| (id.to_string(), doc.to_value()))
                .collect::<BTreeMap<String, Value>>()
        };
        let cnc_profiles = collect(crate::data::Profile::Cnc);
        let fixture_profiles = collect(crate::data::Profile::Fixture);
        let processing_profiles = collect(crate::data::Profile::Machining);
        let toolset_profiles = collect(crate::data::Profile::Toolset);

        let get_id = |key: &str| {
            global_settings
                .get(key)
                .and_then(Value::as_str)
                .map(ToString::to_string)
        };
        let selected_process_profile_id = get_id("selected_process_profile_id");
        let last_edited_process_profile_id = get_id("last_edited_process_profile_id");
        let selected_cnc_profile_id = get_id("selected_cnc_profile_id");
        let selected_fixture_profile_id = get_id("selected_fixture_profile_id");
        let selected_toolset_profile_id = get_id("selected_toolset_profile_id");

        PersistenceState {
            global_settings,
            stock,
            cnc_profiles,
            fixture_profiles,
            processing_profiles,
            toolset_profiles,
            selected_process_profile_id,
            last_edited_process_profile_id,
            selected_cnc_profile_id,
            selected_fixture_profile_id,
            selected_toolset_profile_id,
        }
    }))
}

/// Schema-shaped default settings used when the store has no settings document.
fn default_global_settings() -> Value {
    json!({
        "units": "mm",
        "theme": "Light",
        "selected_process_profile_id": Value::Null,
        "selected_cnc_profile_id": Value::Null,
        "selected_fixture_profile_id": Value::Null,
        "selected_toolset_profile_id": Value::Null,
    })
}

pub fn initialize_ctx(boot: UiLaunchData) {
    // AppData is the single reader/writer of every persisted realm; initialize it
    // first so the launch-time hydrate can source its state from the live store.
    for problem in crate::data::init_appdata() {
        warn!("AppData load: {problem}");
    }

    if let Some(state) = persistence_state_from_appdata() {
        let _ = PERSISTENCE_STATE.set(state);
    }

    let _ = GLOBAL_CTX.set(RwLock::new(AppCtx::from_launch(&boot)));
}

#[allow(dead_code)]
fn persistence_state() -> Option<&'static PersistenceState> {
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
    let result = f(&mut guard);
    let updated_state = guard.app.clone();
    guard.sync_from_app_state(&updated_state);
    result
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

        ctx.app.persist_realms(&[PersistRealm::GlobalSettings]);
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

