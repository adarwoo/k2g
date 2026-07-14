use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use log::warn;
use rhai::{Array, Dynamic, Map};

use crate::app_state::AppState;
use crate::board::BoardSnapshot;
use crate::config::{
    backfill_catalog_fields, ensure_default_files, load_all_configs, normalize_catalog_fields,
    write_embedded_schemas, PersistenceState, load_all_configs_best_effort,
};
use crate::config::yaml_service::parse_yaml_with_schema;
use crate::domain::catalog::{catalog_dir, default_catalogs, Catalog, CatalogManager};
use crate::stitching::stitch_edge_shapes;
use crate::ui::model::{
    AppError, CatalogStockCatalog, GenerationState, JobConfig, JobProfile, MachineProfile, Tool,
    UiLaunchData, UiState, CatalogStockSection, CatalogStockTool, PersistRealm, Theme,
    UnitSystem,
};
use crate::user_path::{
    AppDirs,
    ensure_app_dirs,
};

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

#[allow(dead_code)]
#[derive(Clone)]
pub struct StitchedBoardData {
    pub contour_count: usize,
    pub error_count: usize,
    pub errors: Vec<String>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct AppCtx {
    pub ui: UiState,
    pub state: AppState,
    pub cli_args: Vec<String>,
    pub board_data: Option<BoardSnapshot>,
    pub stitched_board_data: Option<StitchedBoardData>,
    pub catalogs: Vec<CatalogStockCatalog>,
    pub stock: Vec<Tool>,
    pub cncs: Vec<MachineProfile>,
    pub process_profiles: Vec<JobProfile>,
    pub current_project: Option<JobConfig>,
    pub regeneration_status: GenerationState,
    pub kicad_status: String,
    pub issues: Vec<CtxIssue>,
    pub status: BTreeMap<String, String>,
    pub tools_mapping: BTreeMap<u8, Option<String>>,
    pub catalogs_loaded: bool,
}

#[allow(dead_code)]
impl AppCtx {
    fn from_launch(boot: &UiLaunchData) -> Self {
        let ui = UiState::new(
            boot.save_filename_override.clone(),
            boot.board_snapshot.clone(),
        );
        let mut status = BTreeMap::new();
        status.insert(STATUS_KEY_KICAD.to_string(), boot.kicad_status.clone());
        status.insert(
            STATUS_KEY_PROJECT_HAS_BOARD.to_string(),
            boot.board_snapshot.is_some().to_string(),
        );

        let stitched_board_data = boot.board_snapshot.as_ref().map(|board| {
            let stitched = stitch_edge_shapes(&board.edge_shapes);
            StitchedBoardData {
                contour_count: stitched.contours.len(),
                error_count: stitched.errors.len(),
                errors: stitched.errors,
            }
        });

        Self {
            state: app_state_from_ui(&ui),
            ui,
            cli_args: boot.cli_args.clone(),
            board_data: boot.board_snapshot.clone(),
            stitched_board_data,
            catalogs: vec![],
            stock: vec![],
            cncs: vec![],
            process_profiles: vec![],
            current_project: None,
            regeneration_status: GenerationState::Idle,
            kicad_status: boot.kicad_status.clone(),
            issues: vec![],
            status,
            tools_mapping: BTreeMap::new(),
            catalogs_loaded: false,
        }
    }

    fn sync_from_ui_state(&mut self, state: &UiState) {
        let mut next_ui = state.clone();

        // Keep context as the source of truth for lazily-loaded catalogs.
        if self.catalogs_loaded && !self.catalogs.is_empty() && next_ui.catalogs.is_empty() {
            next_ui.catalogs = self.catalogs.clone();
        }

        self.ui = next_ui;
        self.state = app_state_from_ui(&self.ui);
        self.board_data = self.ui.board.clone();
        self.stitched_board_data = self.ui.board.as_ref().map(|board| {
            let stitched = stitch_edge_shapes(&board.edge_shapes);
            StitchedBoardData {
                contour_count: stitched.contours.len(),
                error_count: stitched.errors.len(),
                errors: stitched.errors,
            }
        });
        self.catalogs = self.ui.catalogs.clone();
        if !self.catalogs.is_empty() {
            self.catalogs_loaded = true;
        }
        self.stock = self.ui.tools.clone();
        self.cncs = self.ui.machines.clone();
        self.process_profiles = self.ui.process_profiles.clone();
        self.current_project = Some(self.ui.project_config.clone());
        self.regeneration_status = self.ui.generation_state;
        self.tools_mapping = self.ui
            .rack_slots
            .iter()
            .map(|(slot, cfg)| (*slot, cfg.tool_id.clone()))
            .collect::<BTreeMap<_, _>>();

        self.issues = self
            .ui
            .errors
            .iter()
            .map(issue_from_app_error)
            .collect::<Vec<_>>();

        self.status.insert(
            STATUS_KEY_REGENERATION.to_string(),
            match self.ui.generation_state {
                GenerationState::Idle => "idle",
                GenerationState::Generating => "generating",
                GenerationState::Failed => "failed",
            }
            .to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_HAS_BOARD.to_string(),
            self.ui.board.is_some().to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_SELECTED_PROCESS.to_string(),
            self.ui.selected_process_profile_id.clone().unwrap_or_default(),
        );
    }

    pub fn ensure_catalogs_loaded(&mut self) {
        if self.catalogs_loaded {
            return;
        }

        self.catalogs = load_catalog_index();
        self.ui.catalogs = self.catalogs.clone();
        self.state.catalogs = self.catalogs.clone();
        self.catalogs_loaded = true;
    }

    pub fn refresh_catalogs(&mut self) {
        self.catalogs = load_catalog_index();
        self.ui.catalogs = self.catalogs.clone();
        self.state.catalogs = self.catalogs.clone();
        self.catalogs_loaded = true;
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
            if !self.ui.catalogs.iter().any(|c| c.name == candidate) {
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
            if !self.ui.catalogs.iter().any(|c| c.key == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn import_catalog_text(&mut self, stem: &str, yaml_text: &str) -> Result<String, String> {
        self.ensure_catalogs_loaded();

        let catalog = parse_yaml_with_schema::<Catalog, _>(yaml_text, "catalog.yaml", |json_value| {
            normalize_catalog_fields(json_value, stem, true, true);
        })
            .map_err(|_| "Catalog import failed: invalid YAML or schema".to_string())?;
        let unique_name = self.unique_catalog_name(&catalog.name);
        let key_base = format!("import-{}", slug(stem));
        let unique_key = self.unique_catalog_key(&key_base);
        let stock_catalog = catalog_to_stock_catalog(&unique_key, &unique_name, &catalog, false);
        self.ui.catalogs.push(stock_catalog);
        self.catalogs = self.ui.catalogs.clone();
        self.state.catalogs = self.catalogs.clone();
        Ok(unique_name)
    }

    pub fn remove_catalog(&mut self, catalog_key: &str) -> Result<(), String> {
        self.ensure_catalogs_loaded();

        let Some(entry) = self.ui.catalogs.iter().find(|c| c.key == catalog_key).cloned() else {
            return Err("Catalog not found".to_string());
        };

        if entry.built_in {
            return Err("Built-in catalogs cannot be deleted".to_string());
        }

        self.ui.catalogs.retain(|c| c.key != catalog_key);
        self.catalogs = self.ui.catalogs.clone();
        self.state.catalogs = self.catalogs.clone();
        Ok(())
    }

    pub fn clear_domain(&mut self, domain: &str) {
        self.issues.retain(|issue| issue.domain != domain);
    }

    pub fn set_status(&mut self, key: &str, value: impl Into<String>) {
        self.status.insert(key.to_string(), value.into());
    }

    pub fn as_rhai_ctx(&self) -> Map {
        let mut ctx = Map::new();
        ctx.insert("kicad_status".into(), Dynamic::from(self.kicad_status.clone()));
        ctx.insert("cnc_count".into(), Dynamic::from(self.cncs.len() as i64));
        ctx.insert(
            "process_profile_count".into(),
            Dynamic::from(self.process_profiles.len() as i64),
        );
        ctx.insert("stock_count".into(), Dynamic::from(self.stock.len() as i64));
        ctx.insert("has_board".into(), Dynamic::from(self.board_data.is_some()));

        let status_map = self
            .status
            .iter()
            .map(|(key, value)| {
                let mut item = Map::new();
                item.insert("key".into(), Dynamic::from(key.clone()));
                item.insert("value".into(), Dynamic::from(value.clone()));
                Dynamic::from(item)
            })
            .collect::<Array>();
        ctx.insert("status".into(), Dynamic::from(status_map));

        ctx
    }
}

fn app_state_from_ui(state: &UiState) -> AppState {
    AppState {
        machines: state.machines.clone(),
        selected_machine_id: state.selected_machine_id.clone(),
        fixtures: state.fixtures.clone(),
        selected_fixture_id: state.selected_fixture_id.clone(),
        process_profiles: state.process_profiles.clone(),
        selected_process_profile_id: state.selected_process_profile_id.clone(),
        last_edited_process_profile_id: state.last_edited_process_profile_id.clone(),
        toolsets: state.toolsets.clone(),
        selected_toolset_id: state.selected_toolset_id.clone(),
        catalogs: state.catalogs.clone(),
        tools: state.tools.clone(),
        generation_state: state.generation_state,
        project_config: state.project_config.clone(),
        gcode: state.gcode.clone(),
        gcode_modified: state.gcode_modified,
        rack_slots: state.rack_slots.clone(),
        board: state.board.clone(),
    }
}

fn issue_from_app_error(err: &AppError) -> CtxIssue {
    CtxIssue {
        id: err.id.clone(),
        domain: err.domain.clone(),
        is_error: err.is_error,
        message: err.message.clone(),
        details: err.details.clone(),
        created_ms: now_ms(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

static GLOBAL_CTX: OnceLock<RwLock<AppCtx>> = OnceLock::new();
static PERSISTENCE_STATE: OnceLock<PersistenceState> = OnceLock::new();
static PERSISTENCE_SYNC_ARMED: AtomicBool = AtomicBool::new(false);

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

pub fn sync_ctx_from_ui_state(state: &UiState) {
    with_ctx_mut(|ctx| ctx.sync_from_ui_state(state));
}

pub fn sync_ctx_from_ui_state_and_persist(state: &UiState) {
    // On first sync after startup, only mirror UI into context. This avoids
    // rewriting settings during bootstrap while still keeping context current.
    if !PERSISTENCE_SYNC_ARMED.swap(true, Ordering::SeqCst) {
        sync_ctx_from_ui_state(state);
        return;
    }

    let current_ui = with_ctx(|ctx| ctx.ui.clone());
    let changed_realms = PersistRealm::ALL
        .iter()
        .copied()
        .filter(|realm| current_ui.realm_payload(*realm) != state.realm_payload(*realm))
        .collect::<Vec<_>>();

    if changed_realms.is_empty() {
        sync_ctx_from_ui_state(state);
        return;
    }

    sync_ctx_from_ui_state_and_persist_realms(state, &changed_realms);
}

pub fn sync_ctx_from_ui_state_and_persist_realms(state: &UiState, realms: &[PersistRealm]) {
    // Keep persistence orchestration in context-facing API so the UI only
    // dispatches intents and does not call config/persistence directly.
    state.persist_realms(realms);
    sync_ctx_from_ui_state(state);
}

pub fn apply_ui_command(command: UiCommand) {
    with_ctx_mut(|ctx| {
        match command {
            UiCommand::SetUnitSystem(unit_system) => {
                ctx.ui.unit_system = unit_system;
            }
            UiCommand::ToggleTheme => {
                ctx.ui.theme = if ctx.ui.theme == Theme::Dark {
                    Theme::Light
                } else {
                    Theme::Dark
                };
            }
        }

        let updated_ui = ctx.ui.clone();
        updated_ui.persist_realms(&[PersistRealm::GlobalSettings]);
        ctx.sync_from_ui_state(&updated_ui);
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

fn load_catalog_index() -> Vec<CatalogStockCatalog> {
    let mut source_catalogs: Vec<(String, Catalog, bool)> = Vec::new();

    if let Ok(dir) = catalog_dir() {
        ensure_default_files(&dir, default_catalogs(), "catalog", |path| {
            if let Err(e) = backfill_catalog_fields(path) {
                warn!("Could not backfill catalog '{}': {e}", path.display());
            }
        });
    }

    if let (Ok(mut manager), Ok(dir)) = (CatalogManager::new(), catalog_dir()) {
        let _ = manager.load_dir(&dir);
        source_catalogs = manager
            .catalogs()
            .map(|(stem, catalog)| (stem.to_string(), catalog.clone(), false))
            .collect();
    }

    if source_catalogs.is_empty() {
        let sources = [
            ("kyocera".to_string(), include_str!("../resources/catalogs/kyocera.yaml")),
            ("unionfab".to_string(), include_str!("../resources/catalogs/unionfab.yaml")),
            ("generic".to_string(), include_str!("../resources/catalogs/generic.yaml")),
        ];

        for (stem, text) in sources {
            if let Ok(catalog) = parse_yaml_with_schema::<Catalog, _>(text, "catalog.yaml", |json_value| {
                normalize_catalog_fields(json_value, &stem, true, true);
            }) {
                source_catalogs.push((stem, catalog, true));
            }
        }
    }

    source_catalogs
        .into_iter()
        .map(|(stem, catalog, built_in)| {
            let key = slug(&stem);
            catalog_to_stock_catalog(&key, &catalog.name, &catalog, built_in)
        })
        .collect::<Vec<_>>()
}

fn fallback_persistence_state(app_dirs: &AppDirs) -> PersistenceState {
    load_all_configs_best_effort(app_dirs, &app_dirs.schemas)
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
            let core = tool.to_tool_core();
            let kind = core.kind.catalog_label().to_string();
            let display_tool_name = core.display_name();

            tools.push(CatalogStockTool {
                key: format!("{}::t{}", section_key, tool_idx),
                catalog_name: display_name.to_string(),
                section_name: section.name.clone(),
                display_name: display_tool_name,
                kind,
                diameter: core.diameter,
                point_angle: core.point_angle,
                feed_rate: core.feed_rate,
                spindle_speed: core.spindle_speed,
                sku: core.sku,
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
