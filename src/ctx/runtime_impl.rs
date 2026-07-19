#[allow(dead_code)]
impl AppCtx {
    fn from_launch(boot: &UiLaunchData) -> Self {
        let app = AppState::new(
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
        let job_references = collect_job_references(&app);

        Self {
            app,
            cli_args: boot.cli_args.clone(),
            stitched_board_data,
            job_references,
            kicad_status: boot.kicad_status.clone(),
            issues: vec![],
            status,
            catalogs_loaded: false,
        }
    }

    fn sync_from_app_state(&mut self, state: &AppState) {
        let previous_app = self.app.clone();
        let previous_references = self.job_references.clone();
        let mut next_app = state.clone();
        let board_changed = self.app.board != next_app.board;

        // Keep context as the source of truth for lazily-loaded catalogs.
        if self.catalogs_loaded && !self.app.catalogs.is_empty() && next_app.catalogs.is_empty() {
            next_app.catalogs = self.app.catalogs.clone();
        }

        self.app = next_app;

        if board_changed {
            self.stitched_board_data = self.app.board.as_ref().map(|board| {
                let stitched = stitch_edge_shapes(&board.edge_shapes);
                StitchedBoardData {
                    contour_count: stitched.contours.len(),
                    error_count: stitched.errors.len(),
                    errors: stitched.errors,
                }
            });
        }

        if !self.app.catalogs.is_empty() {
            self.catalogs_loaded = true;
        }

        self.job_references = collect_job_references(&self.app);
        let change_set = collect_mutation_changes(&previous_app, &self.app);

        // Some UI edit paths mutate CNC profiles directly; enforce persistence at sync tail.
        if !change_set.changed_machine_profile_ids.is_empty() {
            log::info!(
                "Detected CNC profile changes at sync tail; persisting cnc_profiles for ids=[{}]",
                change_set
                    .changed_machine_profile_ids
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            );
            self.app.persist_realms(&[PersistRealm::CncProfiles]);
        }

        self.issues = self
            .app
            .errors
            .iter()
            .map(issue_from_app_error)
            .collect::<Vec<_>>();

        self.status.insert(
            STATUS_KEY_REGENERATION.to_string(),
            match self.app.generation_state {
                GenerationState::Idle => "idle",
                GenerationState::Generating => "generating",
                GenerationState::Failed => "failed",
            }
            .to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_HAS_BOARD.to_string(),
            self.app.board.is_some().to_string(),
        );
        self.status.insert(
            STATUS_KEY_PROJECT_SELECTED_PROCESS.to_string(),
            self.app.selected_process_profile_id.clone().unwrap_or_default(),
        );
        self.status.insert(
            STATUS_KEY_GENERATION_MODIFIED_UUIDS.to_string(),
            change_set.modified_uuid_entries().join(","),
        );

        let readiness = evaluate_generation_readiness(&self.app, self.stitched_board_data.as_ref());
        self.status.insert(
            STATUS_KEY_GENERATION_READINESS.to_string(),
            readiness.is_ready.to_string(),
        );
        self.status.insert(
            STATUS_KEY_GENERATION_NOGO_REASONS.to_string(),
            readiness.nogo_reasons.join(" | "),
        );

        if let Some(trigger) = detect_generation_trigger(
            &previous_app,
            &self.app,
            &previous_references,
            &self.job_references,
            &change_set,
        ) {
            let modified = change_set.modified_uuid_entries().join(",");
            log::info!(
                "Generation trigger detected: cause={} readiness={} modified=[{}]",
                trigger.cause_key(),
                readiness.is_ready,
                modified
            );
            self.status.insert(
                STATUS_KEY_GENERATION_LAST_TRIGGER.to_string(),
                trigger.cause_key().to_string(),
            );
            if readiness.is_ready {
                self.report_generation_started(trigger, &change_set);
            } else {
                log::warn!(
                    "Generation not started: cause={} nogo_reasons={} modified=[{}]",
                    trigger.cause_key(),
                    readiness.nogo_reasons.join(" | "),
                    modified
                );
                self.app.log_event(format!(
                    "Generation trigger detected ({}) but not started: {}",
                    trigger.label(),
                    readiness.nogo_reasons.join("; ")
                ));
            }
        }
    }

    fn report_generation_started(
        &mut self,
        trigger: GenerationTriggerCause,
        change_set: &MutationChangeSet,
    ) {
        // Stub phase: report generation start, but do not execute generation yet.
        let modified = change_set.modified_uuid_entries().join(", ");
        log::info!(
            "Generation initiated: cause={} modified=[{}]",
            trigger.cause_key(),
            modified
        );
        self.app.log_event(format!(
            "Generation started ({}) [stub/no-op] modified={}",
            trigger.label(),
            if modified.is_empty() { "none" } else { &modified }
        ));
    }

    pub fn ensure_catalogs_loaded(&mut self) {
        if self.catalogs_loaded {
            return;
        }

        self.app.catalogs = load_catalog_index();
        self.catalogs_loaded = true;
    }

    pub fn refresh_catalogs(&mut self) {
        self.app.catalogs = load_catalog_index();
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
            if !self.app.catalogs.iter().any(|c| c.name == candidate) {
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
            if !self.app.catalogs.iter().any(|c| c.key == candidate) {
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
        self.app.catalogs.push(stock_catalog);
        Ok(unique_name)
    }

    pub fn remove_catalog(&mut self, catalog_key: &str) -> Result<(), String> {
        self.ensure_catalogs_loaded();

        let Some(entry) = self.app.catalogs.iter().find(|c| c.key == catalog_key).cloned() else {
            return Err("Catalog not found".to_string());
        };

        if entry.built_in {
            return Err("Built-in catalogs cannot be deleted".to_string());
        }

        self.app.catalogs.retain(|c| c.key != catalog_key);
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
        ctx.insert("cnc_count".into(), Dynamic::from(self.app.machines.len() as i64));
        ctx.insert(
            "process_profile_count".into(),
            Dynamic::from(self.app.process_profiles.len() as i64),
        );
        ctx.insert("stock_count".into(), Dynamic::from(self.app.tools.len() as i64));
        ctx.insert("has_board".into(), Dynamic::from(self.app.board.is_some()));

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

#[derive(Clone, Copy)]
enum GenerationTriggerCause {
    PcbLoadedOrReloaded,
    SelectedMachiningProfileChanged,
    JobConfigurationChanged,
    StockChanged,
    ReferencedDependencyChanged,
}

impl GenerationTriggerCause {
    fn cause_key(self) -> &'static str {
        match self {
            Self::PcbLoadedOrReloaded => "pcb_reload",
            Self::SelectedMachiningProfileChanged => "profile_select",
            Self::JobConfigurationChanged => "job_config_change",
            Self::StockChanged => "stock_change",
            Self::ReferencedDependencyChanged => "dependency_change",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::PcbLoadedOrReloaded => "PCB loaded/reloaded",
            Self::SelectedMachiningProfileChanged => "machining profile changed",
            Self::JobConfigurationChanged => "job configuration changed",
            Self::StockChanged => "stock changed",
            Self::ReferencedDependencyChanged => "referenced dependency changed",
        }
    }
}

struct GenerationReadiness {
    is_ready: bool,
    nogo_reasons: Vec<String>,
}

#[derive(Default)]
struct MutationChangeSet {
    changed_process_profile_ids: BTreeSet<String>,
    changed_machine_profile_ids: BTreeSet<String>,
    changed_fixture_profile_ids: BTreeSet<String>,
    changed_toolset_profile_ids: BTreeSet<String>,
    changed_tool_ids: BTreeSet<String>,
    changed_job_config: bool,
    changed_selected_process: bool,
}

impl MutationChangeSet {
    fn modified_uuid_entries(&self) -> Vec<String> {
        let mut entries = Vec::new();
        for id in &self.changed_process_profile_ids {
            entries.push(format!("process:{id}"));
        }
        for id in &self.changed_machine_profile_ids {
            entries.push(format!("cnc:{id}"));
        }
        for id in &self.changed_fixture_profile_ids {
            entries.push(format!("fixture:{id}"));
        }
        for id in &self.changed_toolset_profile_ids {
            entries.push(format!("toolset:{id}"));
        }
        for id in &self.changed_tool_ids {
            entries.push(format!("tool:{id}"));
        }
        if self.changed_job_config {
            entries.push("job:config".to_string());
        }
        entries
    }

    fn touches_referenced_dependencies(&self, references: &JobReferences) -> bool {
        if let Some(process_id) = references.process_profile_id.as_ref() {
            if self.changed_process_profile_ids.contains(process_id) {
                return true;
            }
        }
        if let Some(cnc_id) = references.cnc_profile_id.as_ref() {
            if self.changed_machine_profile_ids.contains(cnc_id) {
                return true;
            }
        }
        if let Some(fixture_id) = references.fixture_profile_id.as_ref() {
            if self.changed_fixture_profile_ids.contains(fixture_id) {
                return true;
            }
        }
        if let Some(toolset_id) = references.toolset_profile_id.as_ref() {
            if self.changed_toolset_profile_ids.contains(toolset_id) {
                return true;
            }
        }
        self.changed_tool_ids
            .iter()
            .any(|tool_id| references.referenced_tool_ids.contains(tool_id))
    }
}

fn evaluate_generation_readiness(
    app: &AppState,
    stitched: Option<&StitchedBoardData>,
) -> GenerationReadiness {
    let mut nogo_reasons = Vec::new();

    if app.board.is_none() {
        nogo_reasons.push("PCB data not loaded".to_string());
    }

    match stitched {
        Some(stitched_board) => {
            if stitched_board
                .errors
                .iter()
                .any(|err| err.to_ascii_lowercase().contains("open"))
            {
                nogo_reasons.push("Open contours detected".to_string());
            }
            if stitched_board
                .errors
                .iter()
                .any(|err| err.to_ascii_lowercase().contains("floating island"))
            {
                nogo_reasons.push("Floating island detected".to_string());
            }
            if stitched_board.error_count > 0
                && !nogo_reasons.iter().any(|reason| reason == "Open contours detected")
                && !nogo_reasons.iter().any(|reason| reason == "Floating island detected")
            {
                nogo_reasons.push("Stitching errors detected".to_string());
            }
        }
        None => {
            nogo_reasons.push("Board stitching data unavailable".to_string());
        }
    }

    let Some(profile) = selected_process_profile_from_app(app) else {
        nogo_reasons.push("No machining profile selected".to_string());
        return GenerationReadiness {
            is_ready: false,
            nogo_reasons,
        };
    };

    if !profile.pending_required_fields.is_empty() || !profile.usable {
        nogo_reasons.push("Machining profile has missing required attributes".to_string());
    }

    match app
        .machines
        .iter()
        .find(|machine| machine.id == profile.cnc_profile_id)
    {
        Some(machine) if !machine.pending_required_fields.is_empty() || !machine.usable => {
            nogo_reasons.push("Referenced CNC profile is incomplete".to_string());
        }
        None => {
            nogo_reasons.push("Referenced CNC profile is missing".to_string());
        }
        _ => {}
    }

    match app
        .fixtures
        .iter()
        .find(|fixture| fixture.id == profile.fixture_profile_id)
    {
        Some(fixture) if !fixture.pending_required_fields.is_empty() || !fixture.usable => {
            nogo_reasons.push("Referenced fixture profile is incomplete".to_string());
        }
        None => {
            nogo_reasons.push("Referenced fixture profile is missing".to_string());
        }
        _ => {}
    }

    match app
        .toolsets
        .iter()
        .find(|toolset| toolset.id == profile.toolset_profile_id)
    {
        Some(toolset) if !toolset.pending_required_fields.is_empty() || !toolset.usable => {
            nogo_reasons.push("Referenced toolset profile is incomplete".to_string());
        }
        None => {
            nogo_reasons.push("Referenced toolset profile is missing".to_string());
        }
        _ => {}
    }

    if app.errors.iter().any(|error| error.is_error) {
        nogo_reasons.push("Blocking runtime errors present".to_string());
    }

    GenerationReadiness {
        is_ready: nogo_reasons.is_empty(),
        nogo_reasons,
    }
}

fn detect_generation_trigger(
    previous: &AppState,
    current: &AppState,
    previous_references: &JobReferences,
    current_references: &JobReferences,
    change_set: &MutationChangeSet,
) -> Option<GenerationTriggerCause> {
    if previous.board != current.board {
        return Some(GenerationTriggerCause::PcbLoadedOrReloaded);
    }

    if change_set.changed_selected_process {
        return Some(GenerationTriggerCause::SelectedMachiningProfileChanged);
    }

    if change_set.changed_job_config {
        return Some(GenerationTriggerCause::JobConfigurationChanged);
    }

    if !change_set.changed_tool_ids.is_empty() {
        return Some(GenerationTriggerCause::StockChanged);
    }

    if previous_references != current_references
        || change_set.touches_referenced_dependencies(current_references)
        || referenced_dependency_fingerprint(previous, current_references)
            != referenced_dependency_fingerprint(current, current_references)
    {
        return Some(GenerationTriggerCause::ReferencedDependencyChanged);
    }

    None
}

fn referenced_dependency_fingerprint(app: &AppState, references: &JobReferences) -> String {
    let mut parts = Vec::<String>::new();

    parts.push(format!("selected_process:{}", references.process_profile_id.clone().unwrap_or_default()));

    if let Some(process_id) = references.process_profile_id.as_ref() {
        if let Some(profile) = app
            .process_profiles
            .iter()
            .find(|profile| &profile.id == process_id)
        {
        parts.push(format!(
            "profile:{}",
            process_profile_to_value(profile)
        ));
        }
    }

    if let Some(machine_id) = references.cnc_profile_id.as_ref() {
        if let Some(machine) = app.machines.iter().find(|m| &m.id == machine_id) {
            parts.push(format!("machine:{}", machine_profile_to_value(machine)));
        }
    }

    if let Some(fixture_id) = references.fixture_profile_id.as_ref() {
        if let Some(fixture) = app
            .fixtures
            .iter()
            .find(|fixture| &fixture.id == fixture_id)
        {
            parts.push(format!("fixture:{}", fixture_profile_to_value(fixture)));
        }
    }

    if let Some(toolset_id) = references.toolset_profile_id.as_ref() {
        if let Some(toolset) = app
            .toolsets
            .iter()
            .find(|toolset| &toolset.id == toolset_id)
        {
            parts.push(format!("toolset:{}", toolset_profile_to_value(toolset)));
            let referenced_tools = app
                .tools
                .iter()
                .filter(|tool| references.referenced_tool_ids.contains(&tool.id))
                .cloned()
                .collect::<Vec<_>>();
            parts.push(format!("tools:{}", stock_value_from_tools(&referenced_tools)));
        }
    }

    parts.join("||")
}

fn collect_job_references(app: &AppState) -> JobReferences {
    let mut refs = JobReferences {
        process_profile_id: app.selected_process_profile_id.clone(),
        ..JobReferences::default()
    };

    let Some(process_id) = refs.process_profile_id.as_ref() else {
        return refs;
    };

    let Some(profile) = app
        .process_profiles
        .iter()
        .find(|profile| &profile.id == process_id)
    else {
        return refs;
    };

    refs.cnc_profile_id = Some(profile.cnc_profile_id.clone());
    refs.fixture_profile_id = Some(profile.fixture_profile_id.clone());
    refs.toolset_profile_id = Some(profile.toolset_profile_id.clone());

    if let Some(toolset) = app
        .toolsets
        .iter()
        .find(|toolset| toolset.id == profile.toolset_profile_id)
    {
        refs.referenced_tool_ids = toolset
            .slots
            .values()
            .filter_map(|slot| slot.tool_id.clone())
            .collect::<BTreeSet<_>>();
    }

    refs
}

fn collect_mutation_changes(previous: &AppState, current: &AppState) -> MutationChangeSet {
    MutationChangeSet {
        changed_process_profile_ids: collect_changed_ids(
            &map_process_profiles(previous),
            &map_process_profiles(current),
        ),
        changed_machine_profile_ids: collect_changed_ids(
            &map_machine_profiles(previous),
            &map_machine_profiles(current),
        ),
        changed_fixture_profile_ids: collect_changed_ids(
            &map_fixture_profiles(previous),
            &map_fixture_profiles(current),
        ),
        changed_toolset_profile_ids: collect_changed_ids(
            &map_toolset_profiles(previous),
            &map_toolset_profiles(current),
        ),
        changed_tool_ids: collect_changed_ids(&map_tools(previous), &map_tools(current)),
        changed_job_config: job_config_fingerprint(&previous.project_config)
            != job_config_fingerprint(&current.project_config),
        changed_selected_process: previous.selected_process_profile_id
            != current.selected_process_profile_id,
    }
}

fn collect_changed_ids(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();

    for id in previous.keys().chain(current.keys()) {
        let prev = previous.get(id);
        let curr = current.get(id);
        if prev != curr {
            ids.insert(id.clone());
        }
    }

    ids
}

fn map_process_profiles(app: &AppState) -> BTreeMap<String, String> {
    app.process_profiles
        .iter()
        .map(|profile| (profile.id.clone(), process_profile_to_value(profile).to_string()))
        .collect::<BTreeMap<_, _>>()
}

fn map_machine_profiles(app: &AppState) -> BTreeMap<String, String> {
    app.machines
        .iter()
        .map(|profile| (profile.id.clone(), machine_profile_to_value(profile).to_string()))
        .collect::<BTreeMap<_, _>>()
}

fn map_fixture_profiles(app: &AppState) -> BTreeMap<String, String> {
    app.fixtures
        .iter()
        .map(|profile| (profile.id.clone(), fixture_profile_to_value(profile).to_string()))
        .collect::<BTreeMap<_, _>>()
}

fn map_toolset_profiles(app: &AppState) -> BTreeMap<String, String> {
    app.toolsets
        .iter()
        .map(|profile| (profile.id.clone(), toolset_profile_to_value(profile).to_string()))
        .collect::<BTreeMap<_, _>>()
}

fn map_tools(app: &AppState) -> BTreeMap<String, String> {
    app.tools
        .iter()
        .map(|tool| {
            let one_tool = vec![tool.clone()];
            (tool.id.clone(), stock_value_from_tools(&one_tool).to_string())
        })
        .collect::<BTreeMap<_, _>>()
}

fn job_config_fingerprint(config: &JobConfig) -> String {
    let operations = config
        .selected_operations
        .iter()
        .map(|op| op.label())
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "ops={operations};rot={};tab_count={};tab_width={};tab_width_base={};allow_holes={};drill_then_route={};pilot={};router={};mouse_bites={};mouse_pitch={};mouse_tool={}",
        config.rotation_angle,
        config.tab_count,
        config.tab_width_mm,
        config.tab_width_baseline_mm,
        config.allow_routing_holes,
        config.drill_then_route,
        config.pilot_hole_fallback,
        config.outline_router_tool_id.clone().unwrap_or_default(),
        config.mouse_bites_enabled,
        config.mouse_bite_pitch_mm,
        config.mouse_bite_drill_tool_id.clone().unwrap_or_default(),
    )
}

fn selected_process_profile_from_app(app: &AppState) -> Option<&JobProfile> {
    let selected_id = app.selected_process_profile_id.as_ref()?;
    app.process_profiles
        .iter()
        .find(|profile| &profile.id == selected_id)
}

impl Deref for AppCtx {
    type Target = AppState;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

impl DerefMut for AppCtx {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.app
    }
}

fn issue_from_app_error(err: &AppError) -> CtxIssue {
    CtxIssue {
        id: err.id.clone(),
        domain: err.domain.clone(),
        owner_tag: err.owner_tag.clone(),
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
