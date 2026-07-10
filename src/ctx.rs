use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rhai::{Array, Dynamic, Map};

use crate::board::BoardSnapshot;
use crate::stitching::stitch_edge_shapes;
use crate::ui::model::{
    AppError, CatalogStockCatalog, GenerationState, JobConfig, JobProfile, MachineProfile, Tool,
    UiLaunchData, UiState,
};

pub const STATUS_KEY_REGENERATION: &str = "regeneration.status";
pub const STATUS_KEY_KICAD: &str = "kicad.status";
pub const STATUS_KEY_PROJECT_HAS_BOARD: &str = "project.has_board";
pub const STATUS_KEY_PROJECT_SELECTED_PROCESS: &str = "project.selected_process_profile";

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
        }
    }

    fn sync_from_ui_state(&mut self, state: &UiState) {
        self.ui = state.clone();
        self.board_data = state.board.clone();
        self.stitched_board_data = state.board.as_ref().map(|board| {
            let stitched = stitch_edge_shapes(&board.edge_shapes);
            StitchedBoardData {
                contour_count: stitched.contours.len(),
                error_count: stitched.errors.len(),
                errors: stitched.errors,
            }
        });
        self.catalogs = state.catalogs.clone();
        self.stock = state.tools.clone();
        self.cncs = state.machines.clone();
        self.process_profiles = state.process_profiles.clone();
        self.current_project = Some(state.project_config.clone());
        self.regeneration_status = state.generation_state;
        self.tools_mapping = state
            .rack_slots
            .iter()
            .map(|(slot, cfg)| (*slot, cfg.tool_id.clone()))
            .collect::<BTreeMap<_, _>>();

        self.issues = state
            .errors
            .iter()
            .map(issue_from_app_error)
            .collect::<Vec<_>>();

        self.status.insert(
            STATUS_KEY_REGENERATION.to_string(),
            match state.generation_state {
                GenerationState::Idle => "idle",
                GenerationState::Generating => "generating",
                GenerationState::Failed => "failed",
            }
            .to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_HAS_BOARD.to_string(),
            state.board.is_some().to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_SELECTED_PROCESS.to_string(),
            state.selected_process_profile_id.clone().unwrap_or_default(),
        );
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

pub fn initialize_ctx(boot: UiLaunchData) {
    let _ = GLOBAL_CTX.set(RwLock::new(AppCtx::from_launch(&boot)));
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

#[allow(dead_code)]
pub fn rhai_read_only_ctx() -> Map {
    with_ctx(|ctx| ctx.as_rhai_ctx())
}
