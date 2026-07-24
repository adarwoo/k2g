use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;

use pcb::{BoardSnapshot, KiCad, StitchResult};
use crate::catalog_io::{
    backfill_catalog_fields, ensure_default_files, normalize_catalog_fields,
};
use crate::catalog_io::yaml_service::parse_yaml_with_schema;
use crate::data::model::catalog::{catalog_dir, default_catalogs, Catalog, CatalogManager};
use crate::data::model::state::RackSlot;
use crate::data::model::stock::{stock_value_from_tools, tools_from_stock_value};
use crate::data::model::{
    CascadeDeleteImpact, CatalogStockCatalog, CatalogStockSection, CatalogStockTool,
    CutDepthStrategy, FixtureProfile, JobConfig, JobProfile, MachineProfile,
    ProductionOperation, Side, Tool, ToolPreference, ToolStatus, ToolsetGenerationPolicy,
    ToolsetProfile, UserUnitSystem,
};
// Navigation/shell state lives under the UI layer; the runtime references it
// because `AppState` carries the current screen, theme, and generation status.
use crate::ui::navigation::{
    GenerationState, JobCenterView, PersistRealm, Screen, Theme, UiLaunchData,
};
use units::Length;
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
    SetUnitSystem(UserUnitSystem),
    ToggleTheme,
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

/// Canonical runtime application state.
#[derive(Clone)]
pub struct AppState {
    pub selected_screen: Screen,
    pub selected_job_view: JobCenterView,
    pub unit_system: UserUnitSystem,
    pub theme: Theme,
    pub machines: Vec<MachineProfile>,
    pub selected_machine_id: Option<String>,
    pub fixtures: Vec<FixtureProfile>,
    pub selected_fixture_id: Option<String>,
    pub process_profiles: Vec<JobProfile>,
    /// The machining profile the live job runs (drives generation). Mirrored to
    /// the `job.yaml` singleton.
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
    pub gcode_modified: bool,
    pub suppress_persistence: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    pub board: Option<BoardSnapshot>,
    /// Clean KiCad connection status for the status bar.
    pub kicad_status: String,
}

include!("state.rs");

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
    /// The stitched board model (contours + errors) for the current board,
    /// computed once per acquisition and read by the readiness gate and the
    /// generator. `None` until a board is cached.
    pub stitched_board_data: Option<StitchResult>,
    pub job_references: JobReferences,
    pub status: BTreeMap<String, String>,
    pub catalogs_loaded: bool,
}

include!("orchestration.rs");

include!("catalogs.rs");

include!("generation.rs");

/// Per-step tooling plan for the Job screen's "Tooling" tab.
pub mod tooling;

/// Operation-planner adapter: the in-memory machining plan for the "Machining" tab.
pub mod machining_plan;

/// In-memory capture of `tracing`/`log` output for the Logs screen.
pub mod log_capture;
pub use log_capture::CaptureLayer;

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
    /// The machining profile referenced by the live job singleton (`job.yaml`).
    job_machining_profile: Option<String>,
    /// The board orientation angle (degrees) the live job stores.
    job_board_orientation: i32,
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
        let job_machining_profile = data.job_machining_profile().map(|id| id.to_string());
        let job_board_orientation = data.job_board_orientation();
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
            job_machining_profile,
            job_board_orientation,
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

    // Start the background generation worker now that the global ctx exists (the
    // worker publishes results into it). See `docs/gcode-generation.md` §6.
    start_generation_service();

    // If the launched job is already ready, generate once now — the mutation-driven
    // regeneration trigger never fires at launch, so without this the Code view would
    // sit empty until the first edit. Done via a direct lock (not `with_ctx_mut`) so
    // it does not re-run the whole post-mutation reconciliation for a no-op diff.
    if let Some(lock) = GLOBAL_CTX.get() {
        if let Ok(mut ctx) = lock.write() {
            ctx.kick_initial_generation();
        }
    }
}

/// Connect to KiCad and collect the reachable instance's open board. There is at
/// most one — KiCad serves a single fixed API socket, so a second instance is not
/// addressable and a single instance holds at most one PCB (see the
/// `kicad-multi-instance` reference). Returns a clean connection status for display
/// and the board (if any). Stitching happens once when the board is cached in the
/// ctx (see `sync_after_mutation`), not here.
pub fn acquire_board() -> (String, Option<BoardSnapshot>) {
    // The connection is attempted only here — at startup and from the status-bar
    // Refresh button. Every failure below is otherwise invisible (the return type
    // is a display string + optional board), so log the underlying `PcbError`: it
    // is frequently the only clue a user has for *why* KiCad won't connect (IPC
    // API server disabled, socket/pipe unavailable, version mismatch, no PCB open).
    let client = match KiCad::connect() {
        Ok(client) => client,
        Err(err) => {
            warn!("KiCad connect failed: {err}");
            return ("not connected".to_string(), None);
        }
    };

    let status = match client.version() {
        Ok(version) => format!("KiCad {version}"),
        Err(err) => {
            warn!("KiCad connected but version query failed: {err}");
            "connected".to_string()
        }
    };

    let board = match client.enumerate_pcbs() {
        Ok(pcbs) => match pcbs.into_iter().next() {
            Some(pcb) => match client.collect_snapshot(&pcb) {
                Ok(snapshot) => Some(snapshot),
                Err(err) => {
                    warn!("KiCad board snapshot collection failed: {err}");
                    None
                }
            },
            None => {
                log::info!("KiCad connected but no PCB is open");
                None
            }
        },
        Err(err) => {
            warn!("KiCad PCB enumeration failed: {err}");
            None
        }
    };

    (status, board)
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
    // Snapshot the app *before* the mutation so `sync_after_mutation` sees a real
    // old→new diff. (Cloning after `f` would compare the mutated state to itself,
    // which silently disabled board re-stitching and the regeneration trigger.)
    let previous_app = guard.app.clone();
    let result = f(&mut guard);
    guard.sync_after_mutation(&previous_app);
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


