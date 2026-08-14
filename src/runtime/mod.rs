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
    FixtureProfile, JobConfig, JobProfile, MachineProfile,
    BoardFace, ProductionOperation, Tool, ToolPreference, ToolStatus, ToolsetGenerationPolicy,
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

/// Extension for a saved G-code program. `.nc` is the flavour every common sender
/// recognises (Candle, bCNC, UGS, LinuxCNC); the Save dialog offers the other usual
/// suffixes as filters.
pub const GCODE_FILE_EXTENSION: &str = "nc";

/// Default width of the docked Job column, in pixels. Comfortable for the Code,
/// Tooling and Rack views at the 1400px minimum the dock needs overall.
pub const DEFAULT_JOB_PIN_WIDTH: i64 = 560;

/// How narrow the docked Job column may be dragged. Below this a G-code line wraps and
/// the dock stops being readable.
///
/// There is deliberately **no maximum**. What the split has to protect is the screen on
/// the other side, not the dock, and a fixed ceiling is the wrong tool for it: 1000px
/// was most of a laptop panel and a third of a wide monitor. The layout reserves a fixed
/// width for the screen instead and lets the dock have everything else — see
/// `--job-dock-width` in the theme. A stored width wider than the window will therefore
/// come back as "as wide as it goes", which is what the operator meant by dragging it
/// there.
pub const MIN_JOB_PIN_WIDTH: i64 = 380;

/// Size a fresh install opens the window at, in logical pixels. Wide enough for the
/// docked Job column the layout wants (see the 1250px media query in the theme), and
/// small enough for an ordinary laptop panel — a screen it still overflows is handled
/// at launch by the monitor clamp in [`crate::ui::window_state`].
pub const DEFAULT_WINDOW_WIDTH: i64 = 1400;
pub const DEFAULT_WINDOW_HEIGHT: i64 = 900;

/// Bounds a stored window size is clamped to, matching `schemas/settings.yaml`. The
/// minimum keeps a stale or hand-edited settings file from opening a window too small
/// to operate; the maximum is only a sanity rail, since the real limit is whichever
/// monitor the window reopens on.
pub const MIN_WINDOW_WIDTH: i64 = 640;
pub const MIN_WINDOW_HEIGHT: i64 = 480;
pub const MAX_WINDOW_DIMENSION: i64 = 20000;

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
    /// Which machining step every Job view is showing. In-memory only, like
    /// [`AppState::selected_job_view`], and deliberately absent from the regeneration
    /// fingerprint: looking at another step is not a change to the job. Clamped to the
    /// profile's step count on every mutation (see `sync_after_mutation`), so a removed
    /// step cannot leave it dangling.
    pub selected_step: usize,
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
    /// One program per machining step, in step order — a step owns its CNC, and a CNC
    /// owns the output format, so the steps of one job are separate programs rather than
    /// sections of one. A step that could not be rendered is present and carries its
    /// reason (see [`ProgramOutcome`]) instead of being missing.
    pub programs: Vec<StepProgram>,
    pub suppress_persistence: bool,
    pub show_first_launch: bool,
    pub rack_slots: BTreeMap<u8, RackSlot>,
    pub board: Option<BoardSnapshot>,
    /// Clean KiCad connection status for the status bar.
    pub kicad_status: String,
    /// Directory the last G-code save wrote to, mirrored to `global.setting.yaml`.
    /// `None` until the first save; see [`AppState::gcode_save_directory_or_default`].
    pub gcode_save_directory: Option<String>,
    /// Directory on removable media the last "Save to USB" wrote to. Separate from
    /// [`AppState::gcode_save_directory`] so the two histories cannot contaminate each
    /// other — see [`removable::removable_save_directory`], which resolves it against
    /// the media actually plugged in at the time.
    pub last_removable_media_path: Option<String>,
    /// Keep the Job view docked beside the profile screens (see
    /// [`Screen::shows_pinned_job`]). The flag is kept even while the window is too
    /// narrow to honour it, so widening restores the layout without re-pinning.
    pub job_view_pinned: bool,
    /// Width of the docked Job column in pixels, as left by the split handle.
    pub job_pin_width: i64,
    /// Inner size of the application window in logical pixels, as the user last left
    /// it — restored at the next launch. While the window is maximized these keep the
    /// *restored* size, so un-maximizing gives back the window that was maximized.
    pub window_width: i64,
    pub window_height: i64,
    /// Whether the window was maximized when it was last closed.
    pub window_maximized: bool,
    /// Whether k2g may ask GitHub for a newer release. On by default with a user
    /// opt-out, per EU CRA Annex I (2)(c). This gates the application's *only*
    /// outbound network request — with it off, k2g touches nothing but the local
    /// KiCad socket.
    pub update_check_enabled: bool,
    /// RFC 3339 UTC stamp of the last completed check, holding it to once a day.
    pub update_last_check: Option<String>,
    /// A bare `X.Y.Z` the user chose never to be offered again.
    pub update_skipped_version: Option<String>,
    /// RFC 3339 UTC instant before which no update banner appears ("remind me later").
    pub update_postponed_until: Option<String>,
    /// Whether security-relevant events are appended to `logs/security.jsonl`. On by
    /// default with a user opt-out, per EU CRA Annex I (2)(l). Unrelated to the
    /// in-memory diagnostic log, which is never persisted either way.
    pub security_log_enabled: bool,
    /// A newer release the background check found, if any and if not suppressed.
    /// In-memory only — it is re-derived from GitHub, never persisted.
    pub available_update: Option<update::AvailableUpdate>,
    /// An install is downloading and verifying. Holds the banner's buttons shut so a
    /// second click cannot start a second download over the first.
    pub update_installing: bool,
}

include!("state.rs");

/// Resolved references used by the active job — everything a regeneration must watch.
///
/// The profile ids are **sets over every step**, not one id each. They were single
/// because the job had a single machine, fixture and toolset; a step owns its own, so a
/// second step's CNC went unwatched and editing it changed no program. Same for the
/// tools: the rack is per step, so a tool loaded only by step 2 still belongs here.
#[allow(dead_code)]
#[derive(Clone, Default, PartialEq, Eq)]
pub struct JobReferences {
    pub process_profile_id: Option<String>,
    pub cnc_profile_ids: BTreeSet<String>,
    pub fixture_profile_ids: BTreeSet<String>,
    pub toolset_profile_ids: BTreeSet<String>,
    pub referenced_tool_ids: BTreeSet<String>,
    /// The machining profile exactly as persisted — every step, binding, operation and
    /// per-operation setting.
    ///
    /// The regeneration trigger used to fingerprint the in-memory `JobProfile`, which is
    /// the **step-0 flattened** projection: adding a step, configuring it, or deleting it
    /// changed nothing the trigger could see, so no program was ever regenerated for it.
    /// The document is the only representation that carries all the steps.
    pub machining_document: String,
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
    /// How many mutations this context has been through, stamped on every snapshot.
    ///
    /// A *field*, not a global counter read on demand, and that is the whole point: the
    /// UI holds snapshots that outlive the state they were taken from, and a cache asking
    /// "what is the revision now?" would happily file a plan computed from a stale
    /// snapshot under the current revision — then serve it as though it were fresh.
    pub revision: u64,
    /// Which acquisition of the board this is, counting from launch.
    ///
    /// The board's *name* is not its identity: edit it in KiCad, press Reload PCB, and the
    /// name is what it always was while every track may have moved. Anything caching work
    /// derived from the board keys on this instead.
    pub board_epoch: u64,
    /// Isolation contours for copper engraving, and whether any are being worked out.
    ///
    /// On the context rather than in [`AppState`], which is deep-cloned before every
    /// mutation so the post-mutation sync has something to diff against — and a board's
    /// worth of contours is not something to copy on each keystroke. The result itself is
    /// behind an `Arc` for the same reason.
    pub isolation: isolation::IsolationState,
    /// The last machining plan, shared by every clone of this context.
    ///
    /// Deliberately behind an `Arc` that clones by *sharing* rather than copying: a
    /// snapshot is taken far more often than the job changes, and a per-snapshot cache
    /// would miss on every one of them.
    pub plan_cache: machining_plan::PlanCache,
}

include!("orchestration.rs");

include!("catalogs.rs");

include!("generation.rs");

/// Per-step tooling plan for the Job screen's "Tooling" tab.
pub mod tooling;

/// Operation-planner adapter: the in-memory machining plan for the "Machining" tab.
pub mod machining_plan;

/// Isolation contours for copper engraving, computed on their own worker thread.
pub mod isolation;

/// In-memory capture of `tracing`/`log` output for the Logs screen.
pub mod log_capture;
pub use log_capture::CaptureLayer;

/// Removable media (USB keys, SD cards): detection, save targeting, and eject.
/// Windows-only in substance; the other platforms get a stub with the same API, so the
/// UI needs no `cfg`.
pub mod removable;

/// Registering k2g as a KiCad IPC plugin, and enabling KiCad's API server. Both are
/// user-initiated: nothing here touches KiCad's installation without being asked.
pub mod kicad_integration;

/// The update check — k2g's only outbound network request, off entirely when the
/// user opts out. Downloads are signature-verified before they are ever executed.
pub mod update;

/// The persisted record of security-relevant events (EU CRA Annex I (2)(l)). Local
/// only, opt-out, and path-redacted so it carries no personal data.
pub mod security_log;

/// Resetting k2g to its shipped state, and deleting every trace of it
/// (EU CRA Annex I (2)(b) and (2)(m)).
pub mod data_lifecycle;

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
        "gcode_save_directory": Value::Null,
        "last_removable_media_path": Value::Null,
        "job_view_pinned": false,
        "job_pin_width": DEFAULT_JOB_PIN_WIDTH,
        "window_width": DEFAULT_WINDOW_WIDTH,
        "window_height": DEFAULT_WINDOW_HEIGHT,
        "window_maximized": false,
        "update_check_enabled": true,
        "update_last_check": Value::Null,
        "update_skipped_version": Value::Null,
        "update_postponed_until": Value::Null,
        "security_log_enabled": true,
    })
}

pub fn initialize_ctx(boot: UiLaunchData) {
    // AppData is the single reader/writer of every persisted realm; initialize it
    // first so the launch-time hydrate can source its state from the live store.
    //
    // The problems are held rather than recorded here: the security log must not
    // write anything before the user's opt-out has been read, and the opt-out lives
    // in the settings document this call is what loads. They are recorded a few lines
    // below, once that preference is known.
    let load_problems = crate::data::init_appdata();
    for problem in &load_problems {
        warn!("AppData load: {problem}");
    }

    if let Some(state) = persistence_state_from_appdata() {
        let _ = PERSISTENCE_STATE.set(state);
    }

    let _ = GLOBAL_CTX.set(RwLock::new(AppCtx::from_launch(&boot)));

    // Bring the security-log writer in line with the user's stored preference before
    // anything can try to write, then open the record with the run that is starting.
    // It defaults to on, so a first run is recorded from its first line.
    let recording = with_ctx(|ctx| ctx.security_log_enabled);
    security_log::set_enabled(recording);
    security_log::record_ok(
        security_log::Event::AppStarted,
        json!({
            // Whether KiCad handed us the socket tells the record which way this run
            // was started — from the plugin button, or on its own.
            "launched_by_kicad": std::env::var_os("KICAD_API_SOCKET").is_some(),
        }),
    );

    // A configuration or catalog file that failed validation was not loaded, so the
    // application is running on something other than what is on disk. That is worth
    // being able to find later — it is the difference between "k2g ignored my profile"
    // and "my profile is wrong".
    for problem in &load_problems {
        security_log::record(
            security_log::Event::ConfigRejected,
            security_log::Outcome::Failed,
            json!({ "problem": security_log::redact_str(&problem.to_string()) }),
        );
    }

    // Start the background generation worker now that the global ctx exists (the
    // worker publishes results into it). See `docs/gcode-generation.md` §6.
    start_generation_service();

    // Its own worker, not a share of the generation one: generation is gated on the job
    // being ready to machine, while the views want contours to draw regardless.
    isolation::start_isolation_service();

    // After the generation service, not before: that call is what creates the UI wake
    // channel the watcher bumps. Started earlier, its first scan's wake would silently
    // no-op and an already-inserted stick would stay invisible until the second tick.
    removable::start_removable_media_watcher();

    // Ask GitHub whether a newer release exists — at most once a day, and not at all
    // when the user has opted out. Off the UI thread, and silent unless it finds
    // something (EU CRA Annex I (2)(c)).
    update::start_update_check();

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
/// Prefix of the only [`acquire_board`] status that means KiCad is answering — it is
/// followed by the version it reported. Every other status is a failure state, so the
/// UI tests for this rather than enumerating the failures and missing a new one.
pub const KICAD_STATUS_OK_PREFIX: &str = "KiCad ";

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

    // `connect` only dials the socket, and nng's dial is asynchronous — it succeeds
    // even when nothing is listening, so this is the first call that proves KiCad is
    // actually answering. A failure here therefore means "dialed but not responding",
    // never "connected": reporting the latter sends the user hunting for a board
    // problem when the API server is the thing that is not there.
    let status = match client.version() {
        Ok(version) => format!("{KICAD_STATUS_OK_PREFIX}{version}"),
        Err(err) => {
            warn!("KiCad socket dialled but the version query failed: {err}");
            return ("not responding".to_string(), None);
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

/// Records the window's geometry for the next launch, persisting only what changed.
///
/// `size` is the inner size in logical pixels, or `None` while the window is maximized
/// (see [`AppState::set_window_geometry`]).
///
/// Deliberately *not* routed through [`with_ctx_mut`]: that clones the whole app state
/// and runs the post-mutation sync (board re-stitching, the regeneration trigger) —
/// work a window resize has no business provoking. Nothing in the UI renders from these
/// fields either, so there is no signal to bump. Silent when the ctx is not yet live,
/// so an early event cannot panic the event loop.
pub fn store_window_geometry(size: Option<(i64, i64)>, maximized: bool) {
    let Some(lock) = GLOBAL_CTX.get() else {
        return;
    };
    let Ok(mut guard) = lock.write() else {
        return;
    };
    guard.app.set_window_geometry(size, maximized);
}

pub fn apply_ui_command(command: UiCommand) {
    with_ctx_mut(|ctx| {
        match command {
            UiCommand::SetUnitSystem(unit_system) => {
                ctx.app.unit_system = unit_system;
            }
        }

        ctx.app.persist_realms(&[PersistRealm::GlobalSettings]);
    });
}


