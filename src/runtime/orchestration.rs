#[allow(dead_code)]
impl AppCtx {
    fn from_launch(boot: &UiLaunchData) -> Self {
        let mut app = AppState::new(boot);
        // Validate tool selection up front so an infeasible job reads as not-ready
        // (red pill + banner) from the first frame, not only after the first mutation.
        app.validate_tooling();

        let mut status = BTreeMap::new();
        status.insert(STATUS_KEY_KICAD.to_string(), boot.kicad_status.clone());
        status.insert(
            STATUS_KEY_PROJECT_HAS_BOARD.to_string(),
            boot.board_snapshot.is_some().to_string(),
        );

        // One stitch when the boot board is cached; the full result (contours +
        // errors) is kept for the readiness gate and the generator.
        let stitched_board_data = boot
            .board_snapshot
            .as_ref()
            .map(|board| stitch_edge_shapes(&board.edge_shapes));
        let job_references = collect_job_references(&app);

        // Evaluate the readiness gate at startup too, so the pill/banner reflect a
        // board-less or infeasible job from the very first frame — not only after the
        // first mutation runs `sync_after_mutation`. Without this the gate string is
        // absent and the pill would fall back to stale/placeholder gcode.
        let readiness = evaluate_generation_readiness(&app, stitched_board_data.as_ref());
        status.insert(
            STATUS_KEY_GENERATION_READINESS.to_string(),
            readiness.is_ready.to_string(),
        );
        status.insert(
            STATUS_KEY_GENERATION_NOGO_REASONS.to_string(),
            readiness.nogo_reasons.join(" | "),
        );

        Self {
            app,
            stitched_board_data,
            job_references,
            status,
            catalogs_loaded: false,
        }
    }

    /// Reconcile derived context after a mutation. `previous_app` is the app state
    /// captured *before* the mutation ran (see `with_ctx_mut`), so the diff against
    /// the now-current `self.app` is real — this is what drives board re-stitching
    /// and the regeneration trigger.
    fn sync_after_mutation(&mut self, previous_app: &AppState) {
        let previous_references = self.job_references.clone();
        let board_changed = previous_app.board != self.app.board;

        // Keep context as the source of truth for lazily-loaded catalogs: if the
        // mutation dropped them (a fresh snapshot with none), refill from before.
        if self.catalogs_loaded
            && !previous_app.catalogs.is_empty()
            && self.app.catalogs.is_empty()
        {
            self.app.catalogs = previous_app.catalogs.clone();
        }

        if board_changed {
            // One stitch per board (re)acquisition; the full result — contours
            // included — is cached for the generator and the readiness gate.
            self.stitched_board_data = self
                .app
                .board
                .as_ref()
                .map(|board| stitch_edge_shapes(&board.edge_shapes));
        }

        if !self.app.catalogs.is_empty() {
            self.catalogs_loaded = true;
        }

        self.job_references = collect_job_references(&self.app);
        let change_set = collect_mutation_changes(previous_app, &self.app);

        self.status.insert(
            STATUS_KEY_REGENERATION.to_string(),
            match self.app.generation_state {
                GenerationState::Idle => "idle",
                GenerationState::Running => "running",
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

        // Re-run tool selection so an infeasible job raises a blocking error before
        // readiness is judged (a job with no tooling solution must not read as ready).
        self.app.validate_tooling();

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
            previous_app,
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

    /// A regeneration trigger fired and the readiness gate is open: snapshot the
    /// job input, mark the state Running, and hand the request to the worker
    /// (single-flight; a newer request will cancel this one). See
    /// `docs/gcode-generation.md` §5–6.
    fn report_generation_started(
        &mut self,
        trigger: GenerationTriggerCause,
        _change_set: &MutationChangeSet,
    ) {
        let input = self.build_generation_input();
        self.app.generation_state = GenerationState::Running;
        // No start toast: on a live tool generation fires on every edit, so a
        // per-run toast would spam. The bottom status bar shows "Generating GCode…"
        // (and the pill greys) while Running; only completion/failure notify (§8).
        log::info!("Generation enqueued: cause={}", trigger.cause_key());
        enqueue_generation(input);
    }

    /// Launch-time generation: if the job is already ready, snapshot it and
    /// enqueue one run so the Code view shows a real program immediately, without
    /// waiting for the first mutation trigger (which never fires at startup). A
    /// no-op when the readiness gate is closed — the Code view then shows its
    /// empty state until the job becomes ready.
    fn kick_initial_generation(&mut self) {
        let readiness = evaluate_generation_readiness(&self.app, self.stitched_board_data.as_ref());
        if !readiness.is_ready {
            log::info!(
                "Launch generation skipped — job not ready: {}",
                readiness.nogo_reasons.join("; ")
            );
            return;
        }
        let input = self.build_generation_input();
        self.app.generation_state = GenerationState::Running;
        log::info!("Generation enqueued: cause=launch");
        enqueue_generation(input);
    }

    /// Snapshot the resolved job into an immutable [`GenerationInput`] for the
    /// worker. The Coder never sees the ctx or AppData — only this snapshot.
    fn build_generation_input(&self) -> GenerationInput {
        let process = selected_process_profile_from_app(&self.app);
        let process_profile_name = process
            .map(|profile| profile.name.clone())
            .unwrap_or_default();
        let machine = process.and_then(|profile| {
            self.app
                .machines
                .iter()
                .find(|machine| machine.id == profile.cnc_profile_id)
        });
        let cnc_profile_name = machine.map(|machine| machine.name.clone()).unwrap_or_default();
        // The CNC's preamble templates (the legacy `gcode_header`/`gcode_footer`
        // fields carry the `initialise`/`conclude` primitives — see the crosswalk in
        // `machine_profile_from_value`).
        let initialise_template = machine.map(|machine| machine.gcode_header.clone()).unwrap_or_default();
        let conclude_template = machine.map(|machine| machine.gcode_footer.clone()).unwrap_or_default();

        let operations = self
            .app
            .project_config
            .selected_operations
            .iter()
            .map(|op| op.label().to_string())
            .collect();

        let pcb_filename = self.app.board.as_ref().map(|board| board.name.clone()).unwrap_or_default();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        // TODO: `z_safe` should come from the CNC profile (a field or custom
        // attribute); defaulted until that source exists.
        let z_safe = Length::from_mm(5.0);

        GenerationInput {
            board: self.app.board.clone(),
            stitched: self.stitched_board_data.clone(),
            process_profile_name,
            cnc_profile_name,
            operations,
            initialise_template,
            conclude_template,
            pcb_filename,
            timestamp,
            z_safe,
        }
    }

    pub fn ensure_catalogs_loaded(&mut self) {
        if self.catalogs_loaded {
            return;
        }

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
    stitched: Option<&StitchResult>,
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
            if !stitched_board.errors.is_empty()
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
        config.tab_width.as_mm(),
        config.tab_width_baseline.as_mm(),
        config.allow_routing_holes,
        config.drill_then_route,
        config.pilot_hole_fallback,
        config.outline_router_tool_id.clone().unwrap_or_default(),
        config.mouse_bites_enabled,
        config.mouse_bite_pitch.as_mm(),
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

