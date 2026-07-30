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
        // On screen from the first frame, not only once something is edited: an
        // application that opens refusing to work owes the operator the reason
        // immediately, not after they have gone looking for it.
        app.set_readiness_errors(&readiness.nogo_reasons);

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

        // Keep the viewed step inside the profile. Removing a step, or switching to a
        // profile with fewer, would otherwise leave every Job view pointed at a step that
        // no longer exists — which reads as an empty job rather than as a stale index.
        let step_count = crate::runtime::tooling::step_headers(&self.app).len();
        if previous_app.selected_process_profile_id != self.app.selected_process_profile_id {
            self.app.selected_step = 0;
        }
        self.app.selected_step = self.app.selected_step.min(step_count.saturating_sub(1));

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
        // Republished on every mutation, so the banner tracks the gate as the operator
        // works: each reason disappears the moment the thing it names is fixed, and the
        // banner goes with the last of them.
        self.app.set_readiness_errors(&readiness.nogo_reasons);

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
                // Drop the previous run's programs. They were generated from a job that
                // no longer exists — the trigger says so — and cannot be replaced while
                // the gate is shut. Keeping them left the Save button happily writing
                // last configuration's G-code to a USB stick while the pill beside it
                // read "Not ready", and made the saved step count that of a profile the
                // operator had already changed. `publish_failure` clears for exactly this
                // reason: a live tool must never offer a stale program.
                self.app.programs.clear();
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
    /// The per-step body render context from the step's CNC profile: its operation
    /// primitive templates (the legacy field names carry them — see the crosswalk in
    /// `machine_profile_to_value`), the spindle range for the feed/speed clamp, and the
    /// ATC flag (a manual machine gets an operator prompt). Empty templates when the
    /// step has no resolvable CNC — a case the readiness gate already blocks.
    /// The program context for one step, from **its own** CNC and fixture.
    ///
    /// Both used to be taken from the job-level (step-0 projected) profile, which meant a
    /// second step on a different fixture had its header retract to the first step's safe
    /// height and zero into the first step's work coordinate system — a wrong program, not
    /// merely an inelegant one.
    fn build_program_render_ctx(
        &self,
        raw: &crate::runtime::tooling::StepRaw,
    ) -> crate::gcode::program::ProgramRender {
        let machine = raw
            .cnc_id
            .and_then(|id| self.app.machines.iter().find(|m| m.id == id.to_string()));
        let fixture = raw
            .fixture_id
            .and_then(|id| self.app.fixtures.iter().find(|f| f.id == id.to_string()));

        crate::gcode::program::ProgramRender {
            cnc_name: machine.map(|m| m.name.clone()).unwrap_or_default(),
            program_begin_tpl: machine.map(|m| m.program_begin_tpl.clone()).unwrap_or_default(),
            program_end_tpl: machine.map(|m| m.program_end_tpl.clone()).unwrap_or_default(),
            line_format_tpl: machine.map(|m| m.line_format_tpl.clone()).unwrap_or_default(),
            set_unit_tpl: machine.map(|m| m.set_unit_tpl.clone()).unwrap_or_default(),
            set_origin_tpl: machine.map(|m| m.set_origin_tpl.clone()).unwrap_or_default(),
            comment_tpl: machine.map(|m| m.comment_tpl.clone()).unwrap_or_default(),
            message_tpl: machine.map(|m| m.message_tpl.clone()).unwrap_or_default(),
            pause_tpl: machine.map(|m| m.pause_tpl.clone()).unwrap_or_default(),
            // The fixture's safe travel height, clear of clamps and fixture hardware, per
            // the Z-model. A conservative 5 mm only when no fixture resolves — which the
            // reference check already flags.
            z_safe: fixture.map(|f| f.z_safe).unwrap_or(Length::from_mm(5.0)),
            // Empty when no fixture resolves, which `set_origin` reports as an error —
            // the right outcome, since there is no origin to guess at.
            origin_reference: fixture.map(|f| f.origin_reference.clone()).unwrap_or_default(),
            file_extension: machine
                .map(|m| m.output_file_extension.clone())
                .unwrap_or_else(|| crate::runtime::GCODE_FILE_EXTENSION.to_string()),
            body: Self::build_step_render_ctx(machine),
        }
    }

    fn build_step_render_ctx(machine: Option<&MachineProfile>) -> crate::gcode::program::StepRender {
        use crate::gcode::feeds::{MachineLimits, SpindleRange};
        match machine {
            Some(m) => crate::gcode::program::StepRender {
                drill_tpl: m.drill_tpl.clone(),
                tool_change_tpl: m.tool_change_tpl.clone(),
                tool_measure_tpl: m.tool_measure_tpl.clone(),
                spindle_start_tpl: m.spindle_start_tpl.clone(),
                spindle_stop_tpl: m.spindle_stop_tpl.clone(),
                move_rapid_tpl: m.move_rapid_tpl.clone(),
                cut_linear_tpl: m.cut_linear_tpl.clone(),
                cut_arc_tpl: m.cut_arc_tpl.clone(),
                curve_tolerance: m.curve_tolerance,
                limits: MachineLimits {
                    spindle: SpindleRange::new(m.spindle_rpm_min, m.spindle_rpm_max),
                    max_feed_xy: m.max_feed_xy,
                    max_feed_z: m.max_feed_z,
                },
                is_atc: m.atc_slot_count > 0,
                measures_tool_length: m.measures_tool_length,
            },
            None => crate::gcode::program::StepRender {
                drill_tpl: String::new(),
                tool_change_tpl: String::new(),
                tool_measure_tpl: String::new(),
                spindle_start_tpl: String::new(),
                spindle_stop_tpl: String::new(),
                move_rapid_tpl: String::new(),
                cut_linear_tpl: String::new(),
                cut_arc_tpl: String::new(),
                // The schema's own default, so a step with no resolvable CNC fits curves
                // the way a freshly created profile would rather than to some other number.
                curve_tolerance: units::Length::from_mm(0.01),
                limits: MachineLimits {
                    spindle: SpindleRange::new(
                        units::RotationalSpeed::from_rpm(1_000.0),
                        units::RotationalSpeed::from_rpm(24_000.0),
                    ),
                    // The schema's own default, so a step with no resolvable CNC behaves
                    // like a freshly created profile rather than like an unlimited machine.
                    max_feed_xy: units::FeedRate::from_mm_per_min(5_000.0),
                    max_feed_z: units::FeedRate::from_mm_per_min(5_000.0),
                },
                is_atc: false,
                measures_tool_length: false,
            },
        }
    }

    fn build_generation_input(&self) -> GenerationInput {
        let process = selected_process_profile_from_app(&self.app);
        let process_profile_name = process
            .map(|profile| profile.name.clone())
            .unwrap_or_default();
        let operations = self
            .app
            .project_config
            .selected_operations
            .iter()
            .map(|op| op.label().to_string())
            .collect();

        let filename = self.app.board.as_ref().map(|board| board.name.clone()).unwrap_or_default();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        // The resolved drill plan plus one program context per step (`steps[i]` matches
        // `plan.steps[i]`) and the tool→feed/speed lookup. Built here on the main thread;
        // the worker only renders.
        let plan = machining_plan::plan_machining(self);
        let profile_id = process.and_then(|profile| Uuid::parse_str(&profile.id).ok());
        let steps = profile_id
            .map(|profile_id| {
                tooling::read_steps(profile_id)
                    .iter()
                    .map(|raw| self.build_program_render_ctx(raw))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // The same steps as the operator wrote them, for the program header to read.
        // Enriched here rather than in the reader because only the running app knows what
        // the bound ids are *called* — the datastore holds the binding, not the name.
        let step_values = profile_id
            .map(|profile_id| {
                let mut values = tooling::read_step_values(profile_id);
                for step in &mut values {
                    self.name_step_bindings(step);
                }
                values
            })
            .unwrap_or_default();
        let tool_feeds = self
            .app
            .tools
            .iter()
            .map(|tool| {
                (
                    tool.id.clone(),
                    crate::gcode::program::ToolFeed {
                        name: tool.display_name(),
                        feed: tool.feed_rate,
                        speed: tool.spindle_speed,
                    },
                )
            })
            .collect();

        GenerationInput {
            process_profile_name,
            operations,
            filename,
            timestamp,
            plan,
            steps,
            step_values,
            tool_feeds,
        }
    }

    /// Adds `cnc_name`, `fixture_name` and `toolset_name` beside the ids a step binds.
    ///
    /// A step's `cnc` field is a UUID, which is the truth but is of no use in a header
    /// comment. The resolved name sits beside it rather than replacing it, so the step a
    /// template sees is still exactly the record `machining.yaml` describes, with the
    /// lookup the operator would otherwise have to do by hand already done.
    ///
    /// An unresolvable binding — none chosen, or a profile since deleted — names itself
    /// as the empty string, the same convention [`Self::build_program_render_ctx`] uses.
    /// A missing *name* is not worth failing a program over; a missing *machine* already
    /// fails it, loudly, when there is no template to render.
    /// Each lookup mirrors [`Self::build_program_render_ctx`]'s: the ids compare as
    /// `Uuid::to_string()` on both sides, because that is what the datastore reader emits
    /// for a reference and what the profile list stores for an identity.
    fn name_step_bindings(&self, step: &mut crate::gcode::step_data::StepValue) {
        name_binding(step, "cnc", |id| {
            self.app.machines.iter().find(|m| m.id == id).map(|m| m.name.clone())
        });
        name_binding(step, "fixture", |id| {
            self.app.fixtures.iter().find(|f| f.id == id).map(|f| f.name.clone())
        });
        name_binding(step, "toolset", |id| {
            self.app.toolsets.iter().find(|t| t.id == id).map(|t| t.name.clone())
        });
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
        // Intersections rather than single-id comparisons: any step's machine, fixture
        // or rack changing is a reason to regenerate, not only the first step's.
        if !self.changed_machine_profile_ids.is_disjoint(&references.cnc_profile_ids)
            || !self
                .changed_fixture_profile_ids
                .is_disjoint(&references.fixture_profile_ids)
            || !self
                .changed_toolset_profile_ids
                .is_disjoint(&references.toolset_profile_ids)
        {
            return true;
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
        nogo_reasons.push(incomplete_reason(
            "Machining profile",
            &profile.name,
            &profile.pending_required_fields,
        ));
    }

    match app
        .machines
        .iter()
        .find(|machine| machine.id == profile.cnc_profile_id)
    {
        Some(machine) if !machine.pending_required_fields.is_empty() || !machine.usable => {
            nogo_reasons.push(incomplete_reason(
                "CNC profile",
                &machine.name,
                &machine.pending_required_fields,
            ));
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
            nogo_reasons.push(incomplete_reason(
                "Fixture profile",
                &fixture.name,
                &fixture.pending_required_fields,
            ));
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
            nogo_reasons.push(incomplete_reason(
                "Toolset profile",
                &toolset.name,
                &toolset.pending_required_fields,
            ));
        }
        None => {
            nogo_reasons.push("Referenced toolset profile is missing".to_string());
        }
        _ => {}
    }

    // A bottom-side step is refused outright.
    //
    // `side_to_machine` is settable per step and is reported back in the job sidebar,
    // but nothing in the generator reads it: no geometry is mirrored, so a bottom-side
    // step emits *the top-side program*. That is not a missing feature the operator can
    // work around — it is a confidently wrong answer that scraps the board, and the UI
    // confirms the wrong thing while producing it.
    //
    // So this blocks rather than warns. There is no correct output to fall back to, and
    // a warning on a program that looks entirely plausible is not a safeguard. Lift it
    // when the mirror (and the fixture's `board_flip_axis`) are actually applied.
    if crate::data::appdata_ready() {
        if let Ok(profile_id) = Uuid::parse_str(&profile.id) {
            let steps = crate::runtime::tooling::read_steps(profile_id);
            if let Some(reason) = crate::runtime::tooling::bottom_side_steps_reason(&steps) {
                nogo_reasons.push(reason);
            }
            // Two steps cutting the same feature on the same side. The editor cannot
            // produce this, so reaching here means the profile arrived another way.
            if let Some(reason) = crate::runtime::tooling::duplicate_operations_reason(&steps) {
                nogo_reasons.push(reason);
            }
        }
    }

    if has_blocking_config_error(app) {
        nogo_reasons.push("Blocking runtime errors present".to_string());
    }

    GenerationReadiness {
        is_ready: nogo_reasons.is_empty(),
        nogo_reasons,
    }
}

/// Names an incomplete profile **and the fields it is missing**.
///
/// "Referenced CNC profile is incomplete" was the whole of what an operator got: it did
/// not say which profile, and it did not say what was absent, so the only way to act on it
/// was to open every field of every profile and guess. It also hid a real bug for as long
/// as it existed — the gate was refusing every job over seven fields that were all present
/// under different names, and nothing on screen could have revealed that.
///
/// The field list is the profile's own `pending_required_fields`, i.e. dotted schema paths
/// (`machine.spindle_rpm_min`). Not prose, but it points at exactly one field, which prose
/// would not. Capped so a profile that is missing nearly everything — a hand-written or
/// truncated file — still yields a diagnostic that fits on screen.
fn incomplete_reason(kind: &str, name: &str, missing: &BTreeSet<String>) -> String {
    const MAX_NAMED: usize = 6;

    let named: Vec<&str> = missing.iter().take(MAX_NAMED).map(String::as_str).collect();
    let label = if name.trim().is_empty() {
        kind.to_string()
    } else {
        format!("{kind} '{name}'")
    };
    match (named.as_slice(), missing.len()) {
        // `usable` is derived from the same set being empty, so this is only reachable if
        // that ever stops being true. Still says which profile, which is the half that
        // matters most.
        ([], _) => format!("{label} is incomplete"),
        (_, total) if total > MAX_NAMED => format!(
            "{label} is missing {total} required fields, including {}",
            named.join(", ")
        ),
        _ => format!("{label} is missing {}", named.join(", ")),
    }
}

/// Whether a *configuration* error should hold the generation gate shut.
///
/// Deliberately excludes [`GENERATION_ERROR_DOMAIN`]. Those entries are `is_error` too, but
/// they describe the **previous run's** outcome, and counting them here would make one
/// failure permanent: the entry shuts the gate, the shut gate stops the next run, and only
/// a run can replace the entry. The operator would correct the fixture and watch nothing
/// happen. A failed last run is a reason to *want* the next one.
///
/// [`READINESS_ERROR_DOMAIN`] is excluded for a sharper version of the same reason: those
/// entries *are* this function's own output, published so the operator can see them.
/// Counting them would close the loop on itself — the gate shuts, publishes a reason,
/// reads its own reason back as a blocking error, and stays shut no matter what is fixed.
fn has_blocking_config_error(app: &AppState) -> bool {
    app.errors.iter().any(|error| {
        error.is_error
            && error.domain != GENERATION_ERROR_DOMAIN
            && error.domain != READINESS_ERROR_DOMAIN
    })
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

    // Every step's machine, fixture and rack — not just the first step's. Editing the
    // templates of a CNC that only step 2 runs on must regenerate step 2's program, and
    // the sets are ordered, so the fingerprint is stable.
    for machine in app
        .machines
        .iter()
        .filter(|machine| references.cnc_profile_ids.contains(&machine.id))
    {
        parts.push(format!("machine:{}", machine_profile_to_value(machine)));
    }

    for fixture in app
        .fixtures
        .iter()
        .filter(|fixture| references.fixture_profile_ids.contains(&fixture.id))
    {
        parts.push(format!("fixture:{}", fixture_profile_to_value(fixture)));
    }

    for toolset in app
        .toolsets
        .iter()
        .filter(|toolset| references.toolset_profile_ids.contains(&toolset.id))
    {
        parts.push(format!("toolset:{}", toolset_profile_to_value(toolset)));
    }

    // Once for the job, not once per rack: the id set is already the union across steps,
    // and repeating the same tool per toolset would only make the string longer.
    if !references.referenced_tool_ids.is_empty() {
        let referenced_tools = app
            .tools
            .iter()
            .filter(|tool| references.referenced_tool_ids.contains(&tool.id))
            .cloned()
            .collect::<Vec<_>>();
        parts.push(format!("tools:{}", stock_value_from_tools(&referenced_tools)));
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

    // Step 0's bindings, from the flat projection. Always present, and the only source
    // before the datastore has loaded — the per-step pass below supersedes it.
    refs.cnc_profile_ids.insert(profile.cnc_profile_id.clone());
    refs.fixture_profile_ids.insert(profile.fixture_profile_id.clone());
    refs.toolset_profile_ids.insert(profile.toolset_profile_id.clone());

    // Every *other* step's bindings, and the document that decides them. Read from the
    // datastore because the projection above carries `steps[0]` only.
    if let Ok(profile_uuid) = Uuid::parse_str(process_id) {
        if crate::data::appdata_ready() {
            refs.machining_document = crate::data::with_appdata(|data| {
                data.get(profile_uuid)
                    .map(|doc| doc.to_value().to_string())
                    .unwrap_or_default()
            });
        }
        for step in crate::runtime::tooling::read_steps(profile_uuid) {
            if let Some(cnc) = step.cnc_id {
                refs.cnc_profile_ids.insert(cnc.to_string());
            }
            if let Some(fixture) = step.fixture_id {
                refs.fixture_profile_ids.insert(fixture.to_string());
            }
            if let Some(toolset) = step.toolset_id {
                refs.toolset_profile_ids.insert(toolset.to_string());
            }
        }
    }

    // The tools every referenced rack loads. A tool belonging only to step 2's toolset
    // is as much a dependency as one in step 1's.
    for toolset in app
        .toolsets
        .iter()
        .filter(|toolset| refs.toolset_profile_ids.contains(&toolset.id))
    {
        refs.referenced_tool_ids
            .extend(toolset.slots.values().filter_map(|slot| slot.tool_id.clone()));
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

/// Puts `<field>_name` on a step beside the `<field>` it binds, resolving the id the
/// step holds through `lookup`.
///
/// Free rather than inline so the rule is stated once for all three bindings, and so the
/// part worth testing — the naming convention, and what an unresolved id becomes — needs
/// no profile list to exercise.
///
/// An id that resolves to nothing names itself as the **empty string**, not as an absent
/// field. The difference matters to a template: an absent field is Rhai's `()`, which
/// cannot be interpolated, so a header that names its machine would fail outright on a
/// step whose CNC has since been deleted. That is far too loud for a comment. A missing
/// *machine* already fails the step, where it should, for having no templates to render.
fn name_binding(
    step: &mut crate::gcode::step_data::StepValue,
    field: &str,
    lookup: impl Fn(&str) -> Option<String>,
) {
    use crate::gcode::step_data::StepValue;

    let name = {
        let id = step.field(field).and_then(StepValue::as_text).unwrap_or_default();
        lookup(id).unwrap_or_default()
    };
    step.set_field(&format!("{field}_name"), StepValue::Text(name));
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

/// Named apart from the `tests` module in `generation.rs`, which is `include!`d into
/// this same module and would otherwise collide.
#[cfg(test)]
mod orchestration_tests {
    use super::*;

    fn empty_app() -> AppState {
        AppState::new(&UiLaunchData {
            kicad_status: String::new(),
            board_snapshot: None,
        })
    }

    /// A step's bindings are UUIDs; the header needs what they are called.
    ///
    /// Both halves are pinned. A **bound** id gains the name beside it, with the id left
    /// exactly where it was — the step a template sees is still the record
    /// `machining.yaml` describes, plus a lookup already done. An **unbound** or
    /// dangling one gains an empty name rather than no field at all: an absent field is
    /// Rhai's `()`, and a header naming its machine would then fail outright on a step
    /// whose CNC had been deleted, which is far too loud a failure for a comment.
    #[test]
    fn a_steps_bindings_are_named_beside_the_ids_they_hold() {
        use crate::gcode::step_data::StepValue;

        let mut step = StepValue::Map(vec![
            ("name".into(), StepValue::Text("Drill PTH".into())),
            ("cnc".into(), StepValue::Text("019f-known".into())),
            ("fixture".into(), StepValue::Text("019f-deleted".into())),
        ]);

        let known = |id: &str| (id == "019f-known").then(|| "MASSO G3".to_string());
        name_binding(&mut step, "cnc", known);
        name_binding(&mut step, "fixture", known);
        // No `toolset` field at all — a step that binds none.
        name_binding(&mut step, "toolset", known);

        let text = |field: &str| step.field(field).and_then(StepValue::as_text).map(str::to_string);
        assert_eq!(text("cnc_name"), Some("MASSO G3".to_string()));
        assert_eq!(text("cnc"), Some("019f-known".to_string()), "the id is kept, not replaced");
        assert_eq!(text("fixture_name"), Some(String::new()), "a dangling id names as empty");
        assert_eq!(text("toolset_name"), Some(String::new()), "so does an absent binding");
    }

    /// Editing any step must schedule a regeneration.
    ///
    /// The trigger compares the in-memory `JobProfile`, which is the **step-0 flattened**
    /// projection of the machining profile — it has no steps array at all. So adding a
    /// second step, giving it its CNC and fixture, or deleting it changed nothing the
    /// trigger could see: no program was ever generated for it, `programs` kept the step
    /// count of a profile the operator had already changed, and the Save button offered
    /// one file for a two-step job. The document string is what carries the steps.
    #[test]
    fn a_change_to_a_later_step_triggers_regeneration() {
        let app = empty_app();
        let references = |document: &str| JobReferences {
            process_profile_id: Some("profile".to_string()),
            machining_document: document.to_string(),
            ..JobReferences::default()
        };

        let one_step = references(r#"{"steps":[{"name":"Drill"}]}"#);
        assert!(
            detect_generation_trigger(
                &app,
                &app,
                &one_step,
                &one_step,
                &MutationChangeSet::default()
            )
            .is_none(),
            "an unchanged profile must not regenerate — the trigger fires on every mutation"
        );

        let two_steps = references(r#"{"steps":[{"name":"Drill"},{"name":"Cut out"}]}"#);
        assert!(
            detect_generation_trigger(
                &app,
                &app,
                &one_step,
                &two_steps,
                &MutationChangeSet::default()
            )
            .is_some(),
            "adding a step must regenerate, or its program never exists"
        );

        // Configuring the added step, and deleting it again, are the same kind of change.
        let configured = references(r#"{"steps":[{"name":"Drill"},{"name":"Cut out","cnc":"x"}]}"#);
        assert!(detect_generation_trigger(
            &app,
            &app,
            &two_steps,
            &configured,
            &MutationChangeSet::default()
        )
        .is_some());
        assert!(detect_generation_trigger(
            &app,
            &app,
            &configured,
            &one_step,
            &MutationChangeSet::default()
        )
        .is_some());
    }

    /// A profile referenced only by a later step is still a dependency.
    ///
    /// The reference sets used to be one id each, taken from the step-0 projection, so
    /// editing the CNC that only step 2 runs on regenerated nothing — and the program the
    /// operator then saved was rendered through the *old* templates.
    #[test]
    fn editing_a_profile_only_a_later_step_uses_is_noticed() {
        let mut references = JobReferences {
            process_profile_id: Some("profile".to_string()),
            ..JobReferences::default()
        };
        references.cnc_profile_ids.insert("step-1-cnc".to_string());
        references.cnc_profile_ids.insert("step-2-cnc".to_string());

        let changed_step_2_machine = MutationChangeSet {
            changed_machine_profile_ids: ["step-2-cnc".to_string()].into_iter().collect(),
            ..MutationChangeSet::default()
        };
        assert!(changed_step_2_machine.touches_referenced_dependencies(&references));

        let changed_someone_elses = MutationChangeSet {
            changed_machine_profile_ids: ["unrelated".to_string()].into_iter().collect(),
            ..MutationChangeSet::default()
        };
        assert!(
            !changed_someone_elses.touches_referenced_dependencies(&references),
            "a CNC no step binds is not a reason to regenerate"
        );
    }
}

