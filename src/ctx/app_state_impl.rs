struct RuntimeIssueDraft {
    domain: String,
    owner_tag: Option<String>,
    message: String,
    details: Option<String>,
}

impl AppState {
    // Creates runtime defaults, then hydrates persisted data from disk.
    pub fn new(save_filename_override: Option<String>, board_snapshot: Option<BoardSnapshot>) -> Self {
        let tools = vec![];

        let mut state = Self {
            selected_screen: Screen::Job,
            selected_job_view: JobCenterView::Board,
            unit_system: load_persisted_unit_system(),
            theme: load_persisted_theme(),
            machines: vec![],
            selected_machine_id: None,
            fixtures: vec![],
            selected_fixture_id: None,
            process_profiles: vec![],
            selected_process_profile_id: None,
            last_edited_process_profile_id: None,
            toolsets: vec![],
            selected_toolset_id: None,
            machine_mru: vec![],
            focus_profile_name_editor: false,
            catalogs: vec![],
            tools,
            errors: vec![],
            events: vec![],
            generation_state: GenerationState::Idle,
            project_config: JobConfig {
                selected_operations: vec![ProductionOperation::DrillPth],
                rotation_angle: 0,
                tab_count: 4,
                tab_width_mm: 3.0,
                tab_width_baseline_mm: 3.0,
                allow_routing_holes: true,
                drill_then_route: false,
                pilot_hole_fallback: true,
                outline_router_tool_id: None,
                mouse_bites_enabled: false,
                mouse_bite_pitch_mm: 0.8,
                mouse_bite_drill_tool_id: None,
            },
            gcode: sample_gcode(),
            save_filename: save_filename_override.unwrap_or_else(|| "output.nc".to_string()),
            gcode_modified: false,
            suppress_persistence: false,
            show_first_launch: true,
            rack_slots: BTreeMap::new(),
            board_layers: BoardLayers {
                holes: true,
                routes: true,
                paths: true,
                tabs: true,
            },
            board: board_snapshot,
        };

        state.hydrate_from_persistence();
        if state.rack_slots.is_empty() {
            state.seed_rack_slots(8);
        }
        if state.toolsets.is_empty() {
            state.selected_toolset_id = None;
        }
        if state.selected_toolset_id.is_none() {
            if let Some(toolset) = state.toolsets.first() {
                state.selected_toolset_id = Some(toolset.id.clone());
                state.rack_slots = toolset.slots.clone();
            }
        }
        state
    }

    // Loads persisted domains and resolves cross-domain selections.
    pub fn hydrate_from_persistence(&mut self) {
        self.suppress_persistence = true;

        let Some(persisted) = persistence_state() else {
            self.suppress_persistence = false;
            return;
        };

        let persisted_machines: Vec<MachineProfile> = persisted
            .cnc_profiles
            .values()
            .filter_map(machine_profile_from_value)
            .collect();
        if !persisted_machines.is_empty() {
            self.machines = persisted_machines;
            self.machine_mru.clear();
            self.selected_machine_id = None;
            self.show_first_launch = false;
        }

        let persisted_fixtures: Vec<FixtureProfile> = persisted
            .fixture_profiles
            .values()
            .filter_map(fixture_profile_from_value)
            .collect();
        if !persisted_fixtures.is_empty() {
            self.fixtures = persisted_fixtures;
            self.select_fixture_profile_by_id(
                self.fixtures.first().map(|fixture| fixture.id.clone()),
            );
        }

        let persisted_process_profiles: Vec<JobProfile> = persisted
            .processing_profiles
            .values()
            .filter_map(process_profile_from_value)
            .collect();
        if !persisted_process_profiles.is_empty() {
            self.process_profiles = persisted_process_profiles;
            self.selected_process_profile_id = None;
        }

        let persisted_tools = tools_from_stock_value(&persisted.stock);
        if !persisted_tools.is_empty() {
            self.tools = persisted_tools;
        } else if let Some(disk_tools) = load_tools_direct_from_disk() {
            if !disk_tools.is_empty() {
                self.tools = disk_tools;
            }
        }

        let persisted_toolsets: Vec<ToolsetProfile> = persisted
            .toolset_profiles
            .values()
            .filter_map(toolset_profile_from_value)
            .collect();
        if !persisted_toolsets.is_empty() {
            self.toolsets = persisted_toolsets;
            self.selected_toolset_id = self.toolsets.first().map(|toolset| toolset.id.clone());
            if let Some(toolset) = self.selected_toolset() {
                self.rack_slots = toolset.slots.clone();
            }
        }

        let selected_process = persisted
            .last_edited_process_profile_id
            .clone()
            .filter(|selected| {
                self.process_profiles
                    .iter()
                    .any(|profile| profile.id == *selected)
            });

        let selected_cnc = persisted
            .selected_cnc_profile_id
            .clone()
            .filter(|selected| self.machines.iter().any(|profile| profile.id == *selected));
        let selected_fixture = persisted
            .selected_fixture_profile_id
            .clone()
            .filter(|selected| self.fixtures.iter().any(|profile| profile.id == *selected));
        let selected_toolset = persisted
            .selected_toolset_profile_id
            .clone()
            .filter(|selected| self.toolsets.iter().any(|profile| profile.id == *selected));

        self.last_edited_process_profile_id = selected_process.clone();

        if selected_process.is_some() {
            self.select_process_profile_by_id(selected_process);
        } else {
            let fallback_process = persisted
                .selected_process_profile_id
                .clone()
                .filter(|selected| {
                    self.process_profiles
                        .iter()
                        .any(|profile| profile.id == *selected)
                })
                .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));
            if fallback_process.is_some() {
                self.select_process_profile_by_id(fallback_process);
            } else {
                let selected_machine = selected_cnc
                    .clone()
                    .or_else(|| self.machines.first().map(|machine| machine.id.clone()));
                self.select_machine_profile_by_id(selected_machine);
                if let Some(toolset_id) = selected_toolset
                    .clone()
                    .or_else(|| self.toolsets.first().map(|toolset| toolset.id.clone()))
                {
                    self.select_toolset_profile_by_id(Some(toolset_id));
                }
                self.select_fixture_profile_by_id(
                    selected_fixture
                        .or_else(|| self.fixtures.first().map(|fixture| fixture.id.clone())),
                );
            }
        }

        if self.machines.is_empty() {
            self.show_first_launch = true;
        }

        self.suppress_persistence = false;
    }

    pub fn persist_realms(&self, realms: &[PersistRealm]) {
        if self.suppress_persistence {
            log::debug!(
                "Skipping persistence during startup hydration for realms={:?}",
                realms
            );
            return;
        }

        let Ok(app_dirs) = ensure_app_dirs() else {
            return;
        };

        let persist_processing = realms
            .iter()
            .any(|realm| matches!(realm, PersistRealm::ProcessingProfiles));
        let persist_toolset = realms
            .iter()
            .any(|realm| matches!(realm, PersistRealm::ToolsetProfiles));

        if persist_processing && persist_toolset {
            let processing_profiles = self
                .process_profiles
                .iter()
                .map(|profile| (profile.id.clone(), process_profile_to_value(profile)))
                .collect::<BTreeMap<_, _>>();
            let toolset_profiles = build_toolset_profiles(&self.toolsets);
            match save_processing_and_toolset_profiles_session(
                &app_dirs,
                &processing_profiles,
                &toolset_profiles,
            ) {
                Ok(()) => log::info!(
                    "Persisted processing+toolset session: processing_count={} toolset_count={}",
                    processing_profiles.len(),
                    toolset_profiles.len()
                ),
                Err(err) => log::warn!(
                    "Failed to persist processing+toolset session: {err}"
                ),
            }
        }

        for realm in realms {
            match realm {
                PersistRealm::GlobalSettings => self.persist_global_settings(&app_dirs),
                PersistRealm::CncProfiles => {
                    self.persist_profile_map_realm(
                        "cnc_profiles",
                        &app_dirs,
                        &self.machines,
                        |machine| machine.id.clone(),
                        machine_profile_to_value,
                        save_cnc_profiles,
                    );
                }
                PersistRealm::FixtureProfiles => {
                    self.persist_profile_map_realm(
                        "fixture_profiles",
                        &app_dirs,
                        &self.fixtures,
                        |fixture| fixture.id.clone(),
                        fixture_profile_to_value,
                        save_fixture_profiles,
                    );
                }
                PersistRealm::ProcessingProfiles => {
                    if persist_processing && persist_toolset {
                        continue;
                    }
                    self.persist_profile_map_realm(
                        "processing_profiles",
                        &app_dirs,
                        &self.process_profiles,
                        |profile| profile.id.clone(),
                        process_profile_to_value,
                        save_processing_profiles,
                    );
                }
                PersistRealm::ToolsetProfiles => {
                    if persist_processing && persist_toolset {
                        continue;
                    }
                    self.persist_toolset_profiles(&app_dirs)
                }
                PersistRealm::Stock => self.persist_stock(&app_dirs),
            }
        }
    }

    fn persist_global_settings(&self, app_dirs: &AppDirs) {
        let global_settings = self.make_global_settings_payload();
        match save_global_settings(&app_dirs, &global_settings) {
            Ok(()) => log::info!(
                "Persisted global settings: process={} cnc={} fixture={} toolset={}",
                self.selected_process_profile_id.clone().unwrap_or_default(),
                self.selected_machine_id.clone().unwrap_or_default(),
                self.selected_fixture_id.clone().unwrap_or_default(),
                self.selected_toolset_id.clone().unwrap_or_default(),
            ),
            Err(err) => log::warn!("Failed to persist global settings: {err}"),
        }
    }

    fn make_global_settings_payload(&self) -> Value {
        json!({
            "units": match self.unit_system {
                UnitSystem::Metric => "mm",
                UnitSystem::Imperial => "in",
                UnitSystem::Mil => "mil",
            },
            "theme": match self.theme {
                Theme::Light => "Light",
                Theme::Dark => "Dark",
            },
            "selected_process_profile_id": self.selected_process_profile_id,
            "selected_cnc_profile_id": self.selected_machine_id,
            "selected_fixture_profile_id": self.selected_fixture_id,
            "selected_toolset_profile_id": self.selected_toolset_id,
        })
    }

    fn persist_stock(&self, app_dirs: &AppDirs) {
        let stock = stock_value_from_tools(&self.tools);
        match save_stock(&app_dirs, &stock) {
            Ok(()) => log::info!("Persisted stock: tool_count={}", self.tools.len()),
            Err(err) => log::warn!("Failed to persist stock: {err}"),
        }
    }

    fn persist_stock_snapshot(&self) {
        self.persist_realms(&[PersistRealm::Stock]);
    }

    fn persist_toolset_profiles(&self, app_dirs: &AppDirs) {
        let toolset_profiles = build_toolset_profiles(&self.toolsets);
        match save_toolset_profiles(&app_dirs, &toolset_profiles) {
            Ok(()) => log::info!(
                "Persisted toolset profiles: count={} selected={}",
                toolset_profiles.len(),
                self.selected_toolset_id.clone().unwrap_or_default(),
            ),
            Err(err) => log::warn!("Failed to persist toolset profiles: {err}"),
        }
    }

    fn persist_profile_map_realm<T>(
        &self,
        realm_label: &str,
        app_dirs: &AppDirs,
        items: &[T],
        id_of: impl Fn(&T) -> String,
        to_value: impl Fn(&T) -> Value,
        save_fn: impl Fn(&AppDirs, &BTreeMap<String, Value>) -> Result<(), ConfigError>,
    ) {
        let values = items
            .iter()
            .map(|item| (id_of(item), to_value(item)))
            .collect::<BTreeMap<_, _>>();
        let ids = values.keys().cloned().collect::<Vec<_>>().join(",");
        match save_fn(app_dirs, &values) {
            Ok(()) => log::info!(
                "Persisted realm={} count={} ids=[{}]",
                realm_label,
                values.len(),
                ids
            ),
            Err(err) => log::warn!(
                "Failed to persist realm={} count={} ids=[{}] err={}",
                realm_label,
                values.len(),
                ids,
                err
            ),
        }
    }

    // Runtime event log helper for UI notifications.
    pub fn log_event(&mut self, message: impl Into<String>) {
        let created_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let id = format!("event-{}", Uuid::now_v7());
        self.events.push(AppEvent {
            id,
            message: message.into(),
            created_ms,
        });

        const MAX_EVENT_HISTORY: usize = 200;
        if self.events.len() > MAX_EVENT_HISTORY {
            let drop_count = self.events.len() - MAX_EVENT_HISTORY;
            self.events.drain(0..drop_count);
        }
    }

    fn push_runtime_error_owned(
        &mut self,
        domain: &str,
        owner_tag: Option<String>,
        message: String,
        details: Option<String>,
    ) {
        let created_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        self.errors.push(AppError {
            id: format!("err-{}", created_ms),
            domain: domain.to_string(),
            owner_tag,
            is_error: true,
            message: message.clone(),
            details,
        });
        const MAX_ERRORS: usize = 200;
        if self.errors.len() > MAX_ERRORS {
            let drop_count = self.errors.len() - MAX_ERRORS;
            self.errors.drain(0..drop_count);
        }
        self.log_event(message);
    }

    fn clear_runtime_errors(&mut self, domain: &str) {
        self.errors.retain(|error| error.domain != domain);
    }

    fn clear_runtime_errors_for_owner(&mut self, domain: &str, owner_tag: &str) {
        self.errors.retain(|error| {
            !(error.domain == domain && error.owner_tag.as_deref() == Some(owner_tag))
        });
    }

    fn profile_owner_tag(kind: &str, id: &str) -> String {
        format!("{kind}:{id}")
    }

    pub fn revalidate_current_job_references_for_owner(&mut self, owner_tag: &str) {
        self.clear_runtime_errors_for_owner("current-job-ref", owner_tag);
        for issue in self
            .current_job_reference_errors()
            .into_iter()
            .filter(|issue| issue.owner_tag.as_deref() == Some(owner_tag))
        {
            self.push_runtime_error_owned(
                &issue.domain,
                issue.owner_tag,
                issue.message,
                issue.details,
            );
        }
    }

    pub fn mark_last_edited_process_profile(&mut self, id: Option<String>) {
        self.last_edited_process_profile_id = id;
    }

    pub fn selected_machine(&self) -> Option<&MachineProfile> {
        self.selected_machine_id
            .as_ref()
            .and_then(|id| self.machines.iter().find(|m| &m.id == id))
    }

    pub fn selected_fixture(&self) -> Option<&FixtureProfile> {
        self.selected_fixture_id
            .as_ref()
            .and_then(|id| self.fixtures.iter().find(|fixture| &fixture.id == id))
    }

    pub fn selected_process_profile(&self) -> Option<&JobProfile> {
        self.selected_process_profile_id
            .as_ref()
            .and_then(|id| self.process_profiles.iter().find(|profile| &profile.id == id))
    }

    pub fn selected_toolset(&self) -> Option<&ToolsetProfile> {
        self.selected_toolset_id
            .as_ref()
            .and_then(|id| self.toolsets.iter().find(|toolset| &toolset.id == id))
    }

    pub fn select_toolset_profile_by_id(&mut self, id: Option<String>) {
        let resolved_id = id
            .filter(|selected_id| self.toolsets.iter().any(|toolset| toolset.id == *selected_id))
            .or_else(|| self.toolsets.first().map(|toolset| toolset.id.clone()));

        self.selected_toolset_id = resolved_id.clone();
        if let Some(selected_id) = resolved_id {
            if let Some(toolset) = self.toolsets.iter().find(|toolset| toolset.id == selected_id) {
                self.rack_slots = toolset.slots.clone();
            }
        } else {
            self.rack_slots.clear();
        }
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    pub fn select_fixture_profile_by_id(&mut self, id: Option<String>) {
        let resolved_id = id
            .filter(|selected_id| self.fixtures.iter().any(|fixture| fixture.id == *selected_id))
            .or_else(|| self.fixtures.first().map(|fixture| fixture.id.clone()));

        self.selected_fixture_id = resolved_id;
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    pub fn select_process_profile_by_id(&mut self, id: Option<String>) {
        self.clear_runtime_errors("process-profile");
        self.clear_runtime_errors("current-job-ref");

        let resolved_id = id
            .filter(|selected_id| {
                self.process_profiles
                    .iter()
                    .any(|profile| profile.id == *selected_id)
            })
            .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));

        self.selected_process_profile_id = resolved_id.clone();

        let Some(selected_id) = resolved_id else {
            return;
        };

        let Some(profile) = self
            .process_profiles
            .iter()
            .find(|profile| profile.id == selected_id)
            .cloned()
        else {
            return;
        };

        self.select_machine_profile_by_id(Some(profile.cnc_profile_id.clone()));
        self.selected_fixture_id = Some(profile.fixture_profile_id.clone());
        self.selected_toolset_id = Some(profile.toolset_profile_id.clone());
        if let Some(toolset) = self
            .toolsets
            .iter()
            .find(|toolset| toolset.id == profile.toolset_profile_id)
        {
            self.rack_slots = toolset.slots.clone();
        } else {
            self.rack_slots.clear();
        }

        let ordered_operations = ProductionOperation::all()
            .iter()
            .copied()
            .filter(|op| profile.default_operations.contains(op))
            .collect::<Vec<_>>();
        self.project_config.selected_operations = ordered_operations;
        self.gcode_modified = false;
        self.validate_current_job_references();
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    pub fn validate_current_job_references(&mut self) {
        self.clear_runtime_errors("current-job-ref");

        for issue in self.current_job_reference_errors() {
            self.push_runtime_error_owned(
                &issue.domain,
                issue.owner_tag,
                issue.message,
                issue.details,
            );
        }
    }

    fn current_job_reference_errors(&self) -> Vec<RuntimeIssueDraft> {
        let mut issues = Vec::new();

        let Some(profile) = self.selected_process_profile().cloned() else {
            return issues;
        };

        let process_owner = Some(Self::profile_owner_tag("process", &profile.id));

        if !self.machines.iter().any(|machine| machine.id == profile.cnc_profile_id) {
            issues.push(RuntimeIssueDraft {
                domain: "current-job-ref".to_string(),
                owner_tag: process_owner.clone(),
                message: format!(
                    "Current job cannot execute: broken CNC reference in machining profile '{}'.",
                    profile.name
                ),
                details: Some(format!(
                    "Location: machining profile '{}' -> cnc.default (missing id: {})",
                    profile.name, profile.cnc_profile_id
                )),
            });
        }

        if !self
            .fixtures
            .iter()
            .any(|fixture| fixture.id == profile.fixture_profile_id)
        {
            issues.push(RuntimeIssueDraft {
                domain: "current-job-ref".to_string(),
                owner_tag: process_owner.clone(),
                message: format!(
                    "Current job cannot execute: broken fixture reference in machining profile '{}'.",
                    profile.name
                ),
                details: Some(format!(
                    "Location: machining profile '{}' -> fixture.default (missing id: {})",
                    profile.name, profile.fixture_profile_id
                )),
            });
        }

        if !self
            .toolsets
            .iter()
            .any(|toolset| toolset.id == profile.toolset_profile_id)
        {
            issues.push(RuntimeIssueDraft {
                domain: "current-job-ref".to_string(),
                owner_tag: process_owner.clone(),
                message: format!(
                    "Current job cannot execute: broken toolset reference in machining profile '{}'.",
                    profile.name
                ),
                details: Some(format!(
                    "Location: machining profile '{}' -> toolset.default (missing id: {})",
                    profile.name, profile.toolset_profile_id
                )),
            });
        }

        if let Some(router_id) = self.project_config.outline_router_tool_id.clone() {
            if !self.tools.iter().any(|tool| tool.id == router_id) {
                issues.push(RuntimeIssueDraft {
                    domain: "current-job-ref".to_string(),
                    owner_tag: Some("project:current".to_string()),
                    message: "Current job cannot execute: broken router tool reference.".to_string(),
                    details: Some(format!(
                        "Location: project.outline_router_tool_id (missing id: {})",
                        router_id
                    )),
                });
            }
        }

        if let Some(drill_id) = self.project_config.mouse_bite_drill_tool_id.clone() {
            if !self.tools.iter().any(|tool| tool.id == drill_id) {
                issues.push(RuntimeIssueDraft {
                    domain: "current-job-ref".to_string(),
                    owner_tag: Some("project:current".to_string()),
                    message: "Current job cannot execute: broken mouse-bite drill tool reference."
                        .to_string(),
                    details: Some(format!(
                        "Location: project.mouse_bite_drill_tool_id (missing id: {})",
                        drill_id
                    )),
                });
            }
        }

        if let Some(toolset) = self
            .toolsets
            .iter()
            .find(|toolset| toolset.id == profile.toolset_profile_id)
        {
            let toolset_name = toolset.name.clone();
            let toolset_owner = Some(Self::profile_owner_tag("toolset", &toolset.id));
            let missing_slots = toolset
                .slots
                .iter()
                .filter_map(|(slot_index, slot)| {
                    if !slot.locked || slot.disabled {
                        return None;
                    }
                    let tool_id = slot.tool_id.clone()?;
                    if self.tools.iter().any(|tool| tool.id == tool_id) {
                        None
                    } else {
                        Some((*slot_index, tool_id))
                    }
                })
                .collect::<Vec<_>>();

            for (slot_index, tool_id) in missing_slots {
                issues.push(RuntimeIssueDraft {
                    domain: "current-job-ref".to_string(),
                    owner_tag: toolset_owner.clone(),
                    message: format!(
                        "Current job cannot execute: broken toolset slot reference in '{}'.",
                        toolset_name
                    ),
                    details: Some(format!(
                        "Location: toolset '{}' -> slots.T{} (missing tool id: {})",
                        toolset_name, slot_index, tool_id
                    )),
                });
            }
        }

        issues
    }

    pub fn is_uuid_referenced(&self, uuid: &str) -> bool {
        self.process_profiles.iter().any(|profile| {
            profile.cnc_profile_id == uuid
                || profile.fixture_profile_id == uuid
                || profile.toolset_profile_id == uuid
                || profile.cnc_profile_choices.iter().any(|id| id == uuid)
                || profile.fixture_profile_choices.iter().any(|id| id == uuid)
                || profile.toolset_profile_choices.iter().any(|id| id == uuid)
        }) || self
            .toolsets
            .iter()
            .flat_map(|toolset| toolset.slots.values())
            .any(|slot| slot.tool_id.as_deref() == Some(uuid))
            || self
                .rack_slots
                .values()
                .any(|slot| slot.tool_id.as_deref() == Some(uuid))
            || self
                .project_config
                .outline_router_tool_id
                .as_deref()
                == Some(uuid)
            || self
                .project_config
                .mouse_bite_drill_tool_id
                .as_deref()
                == Some(uuid)
    }

    pub fn current_job_reference_locations_for_uuid(&self, uuid: &str) -> Vec<String> {
        let mut locations = Vec::new();
        let Some(profile) = self.selected_process_profile() else {
            return locations;
        };

        if profile.cnc_profile_id == uuid {
            locations.push(format!(
                "Machining '{}' -> cnc.default",
                profile.name
            ));
        }
        if profile.fixture_profile_id == uuid {
            locations.push(format!(
                "Machining '{}' -> fixture.default",
                profile.name
            ));
        }
        if profile.toolset_profile_id == uuid {
            locations.push(format!(
                "Machining '{}' -> toolset.default",
                profile.name
            ));
        }
        if self.project_config.outline_router_tool_id.as_deref() == Some(uuid) {
            locations.push("Project -> outline router tool".to_string());
        }
        if self.project_config.mouse_bite_drill_tool_id.as_deref() == Some(uuid) {
            locations.push("Project -> mouse-bite drill tool".to_string());
        }
        if let Some(toolset) = self
            .toolsets
            .iter()
            .find(|toolset| toolset.id == profile.toolset_profile_id)
        {
            for (slot_idx, slot) in toolset.slots.iter() {
                if slot.tool_id.as_deref() == Some(uuid) {
                    locations.push(format!(
                        "Toolset '{}' -> slots.T{}",
                        toolset.name, slot_idx
                    ));
                }
            }
        }

        locations
    }

    fn unique_process_profile_name(&self, base_name: &str, exclude_id: Option<&str>) -> String {
        let trimmed = base_name.trim();
        let base = if trimmed.is_empty() {
            "Machining profile".to_string()
        } else {
            trimmed.to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };

            let exists = self
                .process_profiles
                .iter()
                .any(|profile| Some(profile.id.as_str()) != exclude_id && profile.name == candidate);

            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn rename_selected_process_profile(&mut self, new_name: &str) -> Result<String, String> {
        let Some(selected) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        let unique = self.unique_process_profile_name(new_name, Some(selected.as_str()));
        if unique != new_name.trim() {
            return Err(format!("Profile name must be unique. Suggested: {}", unique));
        }

        if let Some(target) = self.process_profiles.iter_mut().find(|profile| profile.id == selected) {
            target.name = unique.clone();
            target.pending_required_fields.remove("name");
            target.usable = target.pending_required_fields.is_empty();
            self.last_edited_process_profile_id = Some(selected);
            self.persist_realms(&[PersistRealm::ProcessingProfiles]);
            return Ok(unique);
        }

        Err("Selected machining profile was not found".to_string())
    }

    pub fn clone_selected_process_profile(&mut self) -> Result<String, String> {
        let Some(current) = self.selected_process_profile().cloned() else {
            return Err("No machining profile selected".to_string());
        };

        let clone_name_seed = format!("{} - copy", current.name.trim());
        let unique_name = self.unique_process_profile_name(&clone_name_seed, None);
        let id = Uuid::now_v7().to_string();

        self.process_profiles.push(JobProfile {
            id: id.clone(),
            name: unique_name,
            cnc_profile_id: current.cnc_profile_id,
            cnc_profile_choices: current.cnc_profile_choices,
            fixture_profile_id: current.fixture_profile_id,
            fixture_profile_choices: current.fixture_profile_choices,
            toolset_profile_id: current.toolset_profile_id,
            toolset_profile_choices: current.toolset_profile_choices,
            side: current.side,
            default_operations: current.default_operations,
            cut_depth_strategy: current.cut_depth_strategy,
            multi_pass_max_depth: current.multi_pass_max_depth,
            operation_setups: current.operation_setups,
            pending_required_fields: current.pending_required_fields,
            usable: current.usable,
        });

        self.select_process_profile_by_id(Some(id.clone()));
        self.last_edited_process_profile_id = Some(id.clone());
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn set_selected_process_profile_cnc(&mut self, cnc_id: &str) -> Result<(), String> {
        self.set_selected_process_profile_cnc_binding(cnc_id, &[cnc_id.to_string()])
    }

    pub fn set_selected_process_profile_cnc_binding(
        &mut self,
        default_id: &str,
        choices: &[String],
    ) -> Result<(), String> {
        if !default_id.trim().is_empty() && !self.machines.iter().any(|machine| machine.id == default_id) {
            return Err("Selected CNC profile was not found".to_string());
        }

        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        if let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        {
            let mut normalized = normalize_binding_choices(choices, default_id);
            normalized.retain(|id| self.machines.iter().any(|machine| machine.id == *id));

            if default_id.trim().is_empty() {
                profile.cnc_profile_id.clear();
            } else {
                profile.cnc_profile_id = default_id.to_string();
                if !normalized.iter().any(|id| id == default_id) {
                    normalized.push(default_id.to_string());
                }
            }

            sort_uuid_v7_ids(&mut normalized);
            profile.cnc_profile_choices = normalized;

            if profile.cnc_profile_id.trim().is_empty() {
                profile.pending_required_fields.insert("cnc.default".to_string());
            } else {
                profile.pending_required_fields.remove("cnc.default");
            }
            if profile.cnc_profile_choices.is_empty() {
                profile.pending_required_fields.insert("cnc.choices".to_string());
            } else {
                profile.pending_required_fields.remove("cnc.choices");
            }
            profile.usable = profile.pending_required_fields.is_empty();
            self.select_process_profile_by_id(Some(selected_id));
            self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
            self.persist_realms(&[PersistRealm::ProcessingProfiles]);
            return Ok(());
        }

        Err("Selected machining profile was not found".to_string())
    }

    #[allow(dead_code)]
    pub fn set_selected_process_profile_fixture(&mut self, fixture_id: &str) -> Result<(), String> {
        self.set_selected_process_profile_fixture_binding(fixture_id, &[fixture_id.to_string()])
    }

    pub fn set_selected_process_profile_fixture_binding(
        &mut self,
        default_id: &str,
        choices: &[String],
    ) -> Result<(), String> {
        if !default_id.trim().is_empty() && !self.fixtures.iter().any(|fixture| fixture.id == default_id) {
            return Err("Selected fixture profile was not found".to_string());
        }

        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        if let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        {
            let mut normalized = normalize_binding_choices(choices, default_id);
            normalized.retain(|id| self.fixtures.iter().any(|fixture| fixture.id == *id));

            if default_id.trim().is_empty() {
                profile.fixture_profile_id.clear();
            } else {
                profile.fixture_profile_id = default_id.to_string();
                if !normalized.iter().any(|id| id == default_id) {
                    normalized.push(default_id.to_string());
                }
            }

            sort_uuid_v7_ids(&mut normalized);
            profile.fixture_profile_choices = normalized;

            if profile.fixture_profile_id.trim().is_empty() {
                profile.pending_required_fields.insert("fixture.default".to_string());
            } else {
                profile.pending_required_fields.remove("fixture.default");
            }
            if profile.fixture_profile_choices.is_empty() {
                profile.pending_required_fields.insert("fixture.choices".to_string());
            } else {
                profile.pending_required_fields.remove("fixture.choices");
            }
            profile.usable = profile.pending_required_fields.is_empty();
            self.select_process_profile_by_id(Some(selected_id));
            self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
            self.persist_realms(&[PersistRealm::ProcessingProfiles]);
            return Ok(());
        }

        Err("Selected machining profile was not found".to_string())
    }

    #[allow(dead_code)]
    pub fn set_selected_process_profile_toolset(&mut self, toolset_id: &str) -> Result<(), String> {
        self.set_selected_process_profile_toolset_binding(toolset_id, &[toolset_id.to_string()])
    }

    pub fn set_selected_process_profile_toolset_binding(
        &mut self,
        default_id: &str,
        choices: &[String],
    ) -> Result<(), String> {
        if !default_id.trim().is_empty() && !self.toolsets.iter().any(|toolset| toolset.id == default_id) {
            return Err("Selected toolset profile was not found".to_string());
        }

        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        if let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        {
            let mut normalized = normalize_binding_choices(choices, default_id);
            normalized.retain(|id| self.toolsets.iter().any(|toolset| toolset.id == *id));

            if default_id.trim().is_empty() {
                profile.toolset_profile_id.clear();
            } else {
                profile.toolset_profile_id = default_id.to_string();
                if !normalized.iter().any(|id| id == default_id) {
                    normalized.push(default_id.to_string());
                }
            }

            sort_uuid_v7_ids(&mut normalized);
            profile.toolset_profile_choices = normalized;

            if profile.toolset_profile_id.trim().is_empty() {
                profile.pending_required_fields.insert("toolset.default".to_string());
            } else {
                profile.pending_required_fields.remove("toolset.default");
            }
            if profile.toolset_profile_choices.is_empty() {
                profile.pending_required_fields.insert("toolset.choices".to_string());
            } else {
                profile.pending_required_fields.remove("toolset.choices");
            }
            profile.usable = profile.pending_required_fields.is_empty();
            self.select_process_profile_by_id(Some(selected_id));
            self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
            self.persist_realms(&[PersistRealm::ProcessingProfiles]);
            return Ok(());
        }

        Err("Selected machining profile was not found".to_string())
    }

    pub fn import_process_profile_yaml(&mut self, yaml: &str) -> Result<String, String> {
        let yaml_value: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|_| "Machining profile import failed: invalid YAML".to_string())?;
        let json_value: Value = serde_json::to_value(yaml_value)
            .map_err(|_| "Machining profile import failed: invalid data".to_string())?;
        let mut profile = process_profile_from_value(&json_value)
            .ok_or_else(|| "Machining profile import failed: non-UUID id/reference detected".to_string())?;

        profile.name = self.unique_process_profile_name(&profile.name, None);
        profile.id = Uuid::now_v7().to_string();

        if !self.machines.iter().any(|machine| machine.id == profile.cnc_profile_id) {
            profile.cnc_profile_id = self
                .selected_machine_id
                .clone()
                .or_else(|| self.machines.first().map(|machine| machine.id.clone()))
                .ok_or_else(|| "Machining profile import failed: no CNC profile is available".to_string())?;
        }
        if profile.cnc_profile_choices.is_empty() {
            profile.cnc_profile_choices = vec![profile.cnc_profile_id.clone()];
        }

        if !self.fixtures.iter().any(|fixture| fixture.id == profile.fixture_profile_id) {
            profile.fixture_profile_id = self
                .selected_fixture_id
                .clone()
                .or_else(|| self.fixtures.first().map(|fixture| fixture.id.clone()))
                .ok_or_else(|| "Machining profile import failed: no fixture profile is available".to_string())?;
        }
        if profile.fixture_profile_choices.is_empty() {
            profile.fixture_profile_choices = vec![profile.fixture_profile_id.clone()];
        }

        if !self.toolsets.iter().any(|toolset| toolset.id == profile.toolset_profile_id) {
            profile.toolset_profile_id = self
                .selected_toolset_id
                .clone()
                .or_else(|| self.toolsets.first().map(|toolset| toolset.id.clone()))
                .ok_or_else(|| "Machining profile import failed: no toolset profile is available".to_string())?;
        }
        if profile.toolset_profile_choices.is_empty() {
            profile.toolset_profile_choices = vec![profile.toolset_profile_id.clone()];
        }

        let selected = profile.id.clone();
        self.process_profiles.push(profile);
        self.select_process_profile_by_id(Some(selected.clone()));
        self.last_edited_process_profile_id = Some(selected.clone());
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(selected)
    }

    pub fn export_selected_process_profile_yaml(&self) -> Result<String, String> {
        let Some(profile) = self.selected_process_profile() else {
            return Err("No machining profile selected".to_string());
        };

        let value = process_profile_to_value(profile);
        let yaml_value: serde_yaml::Value =
            serde_json::from_value(value).map_err(|_| "Export failed: unable to serialize profile".to_string())?;
        serde_yaml::to_string(&yaml_value)
            .map_err(|_| "Export failed: unable to write YAML".to_string())
    }

    fn unique_toolset_name(&self, base_name: &str, exclude_id: Option<&str>) -> String {
        let trimmed = base_name.trim();
        let base = if trimmed.is_empty() {
            "Toolset profile".to_string()
        } else {
            trimmed.to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };

            let exists = self
                .toolsets
                .iter()
                .any(|profile| Some(profile.id.as_str()) != exclude_id && profile.name == candidate);

            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    fn make_toolset_id(&self, _base_name: &str) -> String {
        loop {
            let candidate = Uuid::now_v7().to_string();
            if !self.toolsets.iter().any(|profile| profile.id == candidate) {
                return candidate;
            }
        }
    }

    fn new_toolset_profile(&self, base_name: &str, slot_count: u8) -> ToolsetProfile {
        let unique_name = self.unique_toolset_name(base_name, None);
        let mut defaults = schema_defaults_from_text(TOOLSET_SCHEMA_TEXT);
        if let Some(obj) = defaults.as_object_mut() {
            obj.insert("id".to_string(), Value::String(self.make_toolset_id(&unique_name)));
            obj.insert("name".to_string(), Value::String(unique_name));
            if slot_count > 0 {
                let slots = (1..=slot_count)
                    .map(|idx| {
                        json!({
                            "index": idx,
                            "mode": "spare",
                        })
                    })
                    .collect::<Vec<_>>();
                obj.insert("slots".to_string(), Value::Array(slots));
            }
        }
        toolset_profile_from_value(&defaults).unwrap_or_else(|| ToolsetProfile {
            id: self.make_toolset_id(base_name),
            name: self.unique_toolset_name(base_name, None),
            description: String::new(),
            generation_policy: ToolsetGenerationPolicy::AllowHybrid,
            slots: BTreeMap::new(),
            pending_required_fields: BTreeSet::from(["slots".to_string()]),
            usable: false,
        })
    }

    pub fn add_toolset_profile(&mut self, name: &str) {
        let profile = self.new_toolset_profile(name, 0);
        let selected = profile.id.clone();
        self.rack_slots = profile.slots.clone();
        self.toolsets.push(profile);
        self.select_toolset_profile_by_id(Some(selected));
        self.persist_realms(&[PersistRealm::ToolsetProfiles]);
        if let Some(toolset_id) = self.selected_toolset_id.clone() {
            let owner_tag = Self::profile_owner_tag("toolset", &toolset_id);
            self.revalidate_current_job_references_for_owner(&owner_tag);
        }
    }

    pub fn clone_selected_toolset_profile(&mut self) -> Result<String, String> {
        let Some(current) = self.selected_toolset().cloned() else {
            return Err("No toolset profile selected".to_string());
        };

        let clone_name_seed = format!("{} - copy", current.name.trim());
        let unique_name = self.unique_toolset_name(&clone_name_seed, None);
        let id = self.make_toolset_id(&unique_name);

        self.toolsets.push(ToolsetProfile {
            id: id.clone(),
            name: unique_name,
            description: current.description,
            generation_policy: current.generation_policy,
            slots: current.slots,
            pending_required_fields: current.pending_required_fields,
            usable: current.usable,
        });

        self.select_toolset_profile_by_id(Some(id.clone()));
        self.persist_realms(&[PersistRealm::ToolsetProfiles]);
        let owner_tag = Self::profile_owner_tag("toolset", &id);
        self.revalidate_current_job_references_for_owner(&owner_tag);
        Ok(id)
    }

    pub fn rename_selected_toolset_profile(&mut self, new_name: &str) -> Result<String, String> {
        let Some(selected) = self.selected_toolset_id.clone() else {
            return Err("No toolset profile selected".to_string());
        };

        let unique = self.unique_toolset_name(new_name, Some(selected.as_str()));
        if unique != new_name.trim() {
            return Err(format!("Profile name must be unique. Suggested: {}", unique));
        }

        if let Some(target) = self.toolsets.iter_mut().find(|profile| profile.id == selected) {
            target.name = unique.clone();
            target.pending_required_fields.remove("name");
            target.usable = target.pending_required_fields.is_empty();
            self.persist_realms(&[PersistRealm::ToolsetProfiles]);
            let owner_tag = Self::profile_owner_tag("toolset", &selected);
            self.revalidate_current_job_references_for_owner(&owner_tag);
            return Ok(unique);
        }

        Err("Selected toolset profile was not found".to_string())
    }

    pub fn update_selected_toolset_description(&mut self, description: &str) -> Result<(), String> {
        let Some(selected) = self.selected_toolset_id.clone() else {
            return Err("No toolset profile selected".to_string());
        };

        if let Some(target) = self.toolsets.iter_mut().find(|profile| profile.id == selected) {
            target.description = description.to_string();
            self.persist_realms(&[PersistRealm::ToolsetProfiles]);
            let owner_tag = Self::profile_owner_tag("toolset", &selected);
            self.revalidate_current_job_references_for_owner(&owner_tag);
            return Ok(());
        }

        Err("Selected toolset profile was not found".to_string())
    }

    pub fn set_selected_toolset_generation_policy(&mut self, policy_key: &str) -> Result<(), String> {
        let Some(selected) = self.selected_toolset_id.clone() else {
            return Err("No toolset profile selected".to_string());
        };

        if let Some(target) = self.toolsets.iter_mut().find(|profile| profile.id == selected) {
            target.generation_policy = ToolsetGenerationPolicy::from_key(policy_key);
            target.pending_required_fields.remove("generation_policy");
            target.usable = target.pending_required_fields.is_empty();
            self.persist_realms(&[PersistRealm::ToolsetProfiles]);
            let owner_tag = Self::profile_owner_tag("toolset", &selected);
            self.revalidate_current_job_references_for_owner(&owner_tag);
            return Ok(());
        }

        Err("Selected toolset profile was not found".to_string())
    }

    pub fn set_selected_toolset_slot_mode(
        &mut self,
        slot_index: u8,
        mode: &str,
        tool_id: Option<String>,
    ) -> Result<(), String> {
        let Some(selected) = self.selected_toolset_id.clone() else {
            return Err("No toolset profile selected".to_string());
        };

        let Some(profile) = self.toolsets.iter_mut().find(|profile| profile.id == selected) else {
            return Err("Selected toolset profile was not found".to_string());
        };

        let slot = profile
            .slots
            .entry(slot_index)
            .or_insert(RackSlot {
                tool_id: None,
                locked: false,
                disabled: false,
            });

        match mode {
            "fixed" => {
                slot.disabled = false;
                slot.locked = true;
                slot.tool_id = tool_id;
            }
            "do_not_use" => {
                slot.disabled = true;
                slot.locked = false;
                slot.tool_id = None;
            }
            _ => {
                slot.disabled = false;
                slot.locked = false;
                slot.tool_id = None;
            }
        }

        if !profile.slots.is_empty() {
            profile.pending_required_fields.remove("slots");
        }
        profile.usable = profile.pending_required_fields.is_empty();

        self.rack_slots = profile.slots.clone();
        self.persist_realms(&[PersistRealm::ToolsetProfiles]);
        let owner_tag = Self::profile_owner_tag("toolset", &selected);
        self.revalidate_current_job_references_for_owner(&owner_tag);
        Ok(())
    }

    pub fn set_selected_toolset_slot_count(&mut self, slot_count: u8) -> Result<(), String> {
        let Some(selected) = self.selected_toolset_id.clone() else {
            return Err("No toolset profile selected".to_string());
        };

        let target_count = slot_count.max(1);
        let Some(profile) = self.toolsets.iter_mut().find(|profile| profile.id == selected) else {
            return Err("Selected toolset profile was not found".to_string());
        };

        profile.slots.retain(|slot, _| *slot <= target_count);
        for slot in 1..=target_count {
            profile.slots.entry(slot).or_insert(RackSlot {
                tool_id: None,
                locked: false,
                disabled: false,
            });
        }

        if !profile.slots.is_empty() {
            profile.pending_required_fields.remove("slots");
        }
        profile.usable = profile.pending_required_fields.is_empty();

        self.rack_slots = profile.slots.clone();
        self.persist_realms(&[PersistRealm::ToolsetProfiles]);
        let owner_tag = Self::profile_owner_tag("toolset", &selected);
        self.revalidate_current_job_references_for_owner(&owner_tag);
        Ok(())
    }

    pub fn import_toolset_profile_yaml(&mut self, yaml: &str) -> Result<String, String> {
        let yaml_value: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|_| "Toolset profile import failed: invalid YAML".to_string())?;
        let json_value: Value = serde_json::to_value(yaml_value)
            .map_err(|_| "Toolset profile import failed: invalid data".to_string())?;
        let mut imported = toolset_profile_from_value(&json_value)
            .ok_or_else(|| "Toolset profile import failed: non-UUID id/reference detected".to_string())?;

        imported.name = self.unique_toolset_name(&imported.name, None);
        imported.id = self.make_toolset_id(&imported.name);
        let selected = imported.id.clone();
        self.rack_slots = imported.slots.clone();
        self.toolsets.push(imported);
        self.select_toolset_profile_by_id(Some(selected.clone()));
        self.persist_realms(&[PersistRealm::ToolsetProfiles]);
        let owner_tag = Self::profile_owner_tag("toolset", &selected);
        self.revalidate_current_job_references_for_owner(&owner_tag);
        Ok(selected)
    }

    pub fn export_selected_toolset_yaml(&self) -> Result<String, String> {
        let Some(profile) = self.selected_toolset() else {
            return Err("No toolset profile selected".to_string());
        };

        let value = toolset_profile_to_value(profile);
        let yaml_value: serde_yaml::Value =
            serde_json::from_value(value).map_err(|_| "Export failed: unable to serialize profile".to_string())?;
        serde_yaml::to_string(&yaml_value)
            .map_err(|_| "Export failed: unable to write YAML".to_string())
    }

    pub fn toggle_selected_process_profile_operation(
        &mut self,
        op: ProductionOperation,
    ) -> Result<(), String> {
        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        if let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        {
            if let Some(index) = profile
                .default_operations
                .iter()
                .position(|existing| *existing == op)
            {
                profile.default_operations.remove(index);
                if let Some(setup) = profile.operation_setups.get_mut(operation_to_key(op)) {
                    if let Some(obj) = setup.as_object_mut() {
                        obj.insert("enabled".to_string(), Value::Bool(false));
                    }
                }
            } else {
                profile.default_operations.push(op);
                let entry = profile
                    .operation_setups
                    .entry(operation_to_key(op).to_string())
                    .or_insert_with(|| default_operation_setup_value(op));
                if let Some(obj) = entry.as_object_mut() {
                    obj.insert("enabled".to_string(), Value::Bool(true));
                }
            }

            if !profile.default_operations.is_empty() {
                profile.pending_required_fields.remove("operations");
            }
            profile.usable = profile.pending_required_fields.is_empty();

            self.select_process_profile_by_id(Some(selected_id));
            self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
            self.persist_realms(&[PersistRealm::ProcessingProfiles]);
            return Ok(());
        }

        Err("Selected machining profile was not found".to_string())
    }

    pub fn set_selected_process_profile_side(&mut self, side: Side) -> Result<(), String> {
        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        else {
            return Err("Selected machining profile was not found".to_string());
        };

        profile.side = side;
        self.select_process_profile_by_id(Some(selected_id));
        self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(())
    }

    pub fn set_selected_process_profile_cut_depth_strategy(
        &mut self,
        strategy: CutDepthStrategy,
    ) -> Result<(), String> {
        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        else {
            return Err("Selected machining profile was not found".to_string());
        };

        profile.cut_depth_strategy = strategy;
        self.select_process_profile_by_id(Some(selected_id));
        self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(())
    }

    pub fn set_selected_process_profile_multi_pass_max_depth_mm(
        &mut self,
        depth_mm: f32,
    ) -> Result<(), String> {
        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        else {
            return Err("Selected machining profile was not found".to_string());
        };

        profile.multi_pass_max_depth = units::Length::from_mm(depth_mm.max(0.01) as f64);
        self.select_process_profile_by_id(Some(selected_id));
        self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(())
    }

    pub fn set_selected_process_operation_bool(
        &mut self,
        op: ProductionOperation,
        path: &[&str],
        value: bool,
    ) -> Result<(), String> {
        self.set_selected_process_operation_value(op, path, Value::Bool(value))
    }

    pub fn set_selected_process_operation_string(
        &mut self,
        op: ProductionOperation,
        path: &[&str],
        value: String,
    ) -> Result<(), String> {
        self.set_selected_process_operation_value(op, path, Value::String(value))
    }

    pub fn set_selected_process_operation_u64(
        &mut self,
        op: ProductionOperation,
        path: &[&str],
        value: u64,
    ) -> Result<(), String> {
        self.set_selected_process_operation_value(op, path, Value::Number(value.into()))
    }

    fn set_selected_process_operation_value(
        &mut self,
        op: ProductionOperation,
        path: &[&str],
        value: Value,
    ) -> Result<(), String> {
        if path.is_empty() {
            return Err("Operation config path is empty".to_string());
        }

        let Some(selected_id) = self.selected_process_profile_id.clone() else {
            return Err("No machining profile selected".to_string());
        };

        let Some(profile) = self
            .process_profiles
            .iter_mut()
            .find(|profile| profile.id == selected_id)
        else {
            return Err("Selected machining profile was not found".to_string());
        };

        let setup = profile
            .operation_setups
            .entry(operation_to_key(op).to_string())
            .or_insert_with(|| default_operation_setup_value(op));

        set_nested_value(setup, path, value);

        if !profile.default_operations.iter().any(|existing| *existing == op) {
            profile.default_operations.push(op);
        }

        if let Some(obj) = setup.as_object_mut() {
            obj.insert("enabled".to_string(), Value::Bool(true));
        }

        if !profile.default_operations.is_empty() {
            profile.pending_required_fields.remove("operations");
        }
        profile.usable = profile.pending_required_fields.is_empty();

        self.select_process_profile_by_id(Some(selected_id));
        self.last_edited_process_profile_id = self.selected_process_profile_id.clone();
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        Ok(())
    }

    pub fn selected_machine_has_atc(&self) -> bool {
        self.selected_machine()
            .map(|m| m.atc_slot_count > 0)
            .unwrap_or(false)
    }

    fn make_machine_id(&self, _base_name: &str) -> String {
        loop {
            let candidate = Uuid::now_v7().to_string();
            if !self.machines.iter().any(|m| m.id == candidate) {
                return candidate;
            }
        }
    }

    pub fn unique_machine_name(&self, base_name: &str, exclude_id: Option<&str>) -> String {
        let trimmed = base_name.trim();
        let base = if trimmed.is_empty() {
            "CNC profile".to_string()
        } else {
            trimmed.to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };

            let exists = self
                .machines
                .iter()
                .any(|m| Some(m.id.as_str()) != exclude_id && m.name == candidate);

            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn unique_copy_name(&self, source_name: &str) -> String {
        let base = format!("{} - copy", source_name.trim());
        let first = self.unique_machine_name(&base, None);
        if first == base {
            return first;
        }

        let mut index = 2usize;
        loop {
            let candidate = format!("{} - copy ({})", source_name.trim(), index);
            if !self.machines.iter().any(|m| m.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn select_machine_profile_by_id(&mut self, id: Option<String>) {
        self.selected_machine_id = id.clone();
        if let Some(id) = id {
            self.machine_mru.retain(|m| m != &id);
            self.machine_mru.insert(0, id);
        }
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    pub fn add_machine_profile(&mut self, mut profile: MachineProfile) {
        profile.built_in = false;
        profile.name = self.unique_machine_name(&profile.name, None);
        profile.id = self.make_machine_id(&profile.name);
        let selected = profile.id.clone();
        self.machines.push(profile.clone());
        self.seed_rack_slots(profile.atc_slot_count);
        self.show_first_launch = false;
        self.select_machine_profile_by_id(Some(selected));

        self.persist_realms(&[
            PersistRealm::CncProfiles,
            PersistRealm::ProcessingProfiles,
            PersistRealm::ToolsetProfiles,
            PersistRealm::GlobalSettings,
        ]);
    }

    pub fn add_machine_profile_from_schema(&mut self, name: &str) {
        let mut defaults = schema_defaults_from_text(CNC_SCHEMA_TEXT);
        let unique_name = self.unique_machine_name(name, None);
        if let Some(obj) = defaults.as_object_mut() {
            obj.insert("id".to_string(), Value::String(Uuid::now_v7().to_string()));
            obj.insert("name".to_string(), Value::String(unique_name));
        }

        if let Some(profile) = machine_profile_from_value(&defaults) {
            self.add_machine_profile(profile);
        }
    }

    pub fn rename_selected_machine(&mut self, new_name: &str) -> Result<String, String> {
        let Some(selected) = self.selected_machine_id.clone() else {
            return Err("No CNC profile selected".to_string());
        };

        let unique = self.unique_machine_name(new_name, Some(selected.as_str()));
        if unique != new_name.trim() {
            return Err(format!("Profile name must be unique. Suggested: {}", unique));
        }

        if let Some(target) = self.machines.iter_mut().find(|m| m.id == selected) {
            target.name = unique.clone();
            let renamed = target.name.clone();
            self.persist_realms(&[PersistRealm::CncProfiles]);
            return Ok(renamed);
        }

        Err("Selected CNC profile was not found".to_string())
    }

    #[allow(dead_code)]
    pub fn add_demo_machine(&mut self) {
        let machine = MachineProfile {
            name: format!("Demo CNC profile {}", self.machines.len() + 1),
            max_feed_rate: units::FeedRate::from_mm_per_min(2000.0),
            spindle_rpm_min: units::RotationalSpeed::from_rpm(5000.0),
            spindle_rpm_max: units::RotationalSpeed::from_rpm(24000.0),
            atc_slot_count: 8,
            ..MachineProfile::default()
        };

        self.add_machine_profile(machine);
    }

    pub fn clone_selected_machine(&mut self) {
        let Some(current) = self.selected_machine().cloned() else {
            return;
        };

        let name = self.unique_copy_name(&current.name);
        let clone = MachineProfile {
            id: String::new(),
            name,
            built_in: false,
            ..current
        };

        self.add_machine_profile(clone);
        self.focus_profile_name_editor = true;
    }

    pub fn add_fixture_profile(&mut self, name: &str) {
        let unique_name = self.unique_fixture_name(name, None);
        let fixture_id = Uuid::now_v7().to_string();
        let mut defaults = schema_defaults_from_text(FIXTURE_SCHEMA_TEXT);
        if let Some(obj) = defaults.as_object_mut() {
            obj.insert("id".to_string(), Value::String(fixture_id.clone()));
            obj.insert("name".to_string(), Value::String(unique_name));
        }
        if let Some(profile) = fixture_profile_from_value(&defaults) {
            self.fixtures.push(profile);
            self.selected_fixture_id = Some(fixture_id);
            self.persist_realms(&[PersistRealm::FixtureProfiles]);
        }
    }

    fn unique_fixture_name(&self, base_name: &str, exclude_id: Option<&str>) -> String {
        let trimmed = base_name.trim();
        let base = if trimmed.is_empty() {
            "Fixture profile".to_string()
        } else {
            trimmed.to_string()
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} ({})", base, index)
            };

            let exists = self
                .fixtures
                .iter()
                .any(|fixture| Some(fixture.id.as_str()) != exclude_id && fixture.name == candidate);

            if !exists {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn clone_selected_fixture_profile(&mut self) -> Result<String, String> {
        let Some(current) = self.selected_fixture().cloned() else {
            return Err("No fixture profile selected".to_string());
        };

        let clone_name_seed = format!("{} - copy", current.name.trim());
        let unique_name = self.unique_fixture_name(&clone_name_seed, None);
        let fixture_id = Uuid::now_v7().to_string();

        self.fixtures.push(FixtureProfile {
            id: fixture_id.clone(),
            name: unique_name,
            coordinate_context: current.coordinate_context,
            backing_board: current.backing_board,
            pending_required_fields: current.pending_required_fields,
            usable: current.usable,
        });

        self.selected_fixture_id = Some(fixture_id.clone());
        self.persist_realms(&[PersistRealm::FixtureProfiles]);
        Ok(fixture_id)
    }

    pub fn rename_selected_fixture_profile(&mut self, new_name: &str) -> Result<String, String> {
        let Some(selected) = self.selected_fixture_id.clone() else {
            return Err("No fixture profile selected".to_string());
        };

        let unique = self.unique_fixture_name(new_name, Some(selected.as_str()));
        if unique != new_name.trim() {
            return Err(format!("Profile name must be unique. Suggested: {}", unique));
        }

        if let Some(target) = self.fixtures.iter_mut().find(|fixture| fixture.id == selected) {
            target.name = unique.clone();
            target.pending_required_fields.remove("name");
            target.usable = target.pending_required_fields.is_empty();
            self.persist_realms(&[PersistRealm::FixtureProfiles]);
            return Ok(unique);
        }

        Err("Selected fixture profile was not found".to_string())
    }

    pub fn update_selected_fixture_coordinate_context(&mut self, value: &str) -> Result<(), String> {
        let Some(selected) = self.selected_fixture_id.clone() else {
            return Err("No fixture profile selected".to_string());
        };

        if let Some(target) = self.fixtures.iter_mut().find(|fixture| fixture.id == selected) {
            target.coordinate_context = value.to_string();
            if !value.trim().is_empty() {
                target.pending_required_fields.remove("work_origin_reference");
                target.pending_required_fields.remove("work_origin_reference.z0_reference");
            }
            target.usable = target.pending_required_fields.is_empty();
            self.persist_realms(&[PersistRealm::FixtureProfiles]);
            return Ok(());
        }

        Err("Selected fixture profile was not found".to_string())
    }

    pub fn update_selected_fixture_backing_board(&mut self, value: &str) -> Result<(), String> {
        let Some(selected) = self.selected_fixture_id.clone() else {
            return Err("No fixture profile selected".to_string());
        };

        if let Some(target) = self.fixtures.iter_mut().find(|fixture| fixture.id == selected) {
            target.backing_board = value.to_string();
            if !value.trim().is_empty() {
                target.pending_required_fields.remove("board_holding_method");
            }
            target.usable = target.pending_required_fields.is_empty();
            self.persist_realms(&[PersistRealm::FixtureProfiles]);
            return Ok(());
        }

        Err("Selected fixture profile was not found".to_string())
    }

    pub fn import_fixture_profile_yaml(&mut self, yaml: &str) -> Result<String, String> {
        let yaml_value: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|_| "Fixture profile import failed: invalid YAML".to_string())?;
        let json_value: Value = serde_json::to_value(yaml_value)
            .map_err(|_| "Fixture profile import failed: invalid data".to_string())?;
        let mut profile = fixture_profile_from_value(&json_value)
            .ok_or_else(|| "Fixture profile import failed: non-UUID id detected".to_string())?;

        profile.name = self.unique_fixture_name(&profile.name, None);
        profile.id = Uuid::now_v7().to_string();
        let selected = profile.id.clone();
        self.fixtures.push(profile);
        self.selected_fixture_id = Some(selected.clone());
        self.persist_realms(&[PersistRealm::FixtureProfiles]);
        Ok(selected)
    }

    pub fn export_selected_fixture_yaml(&self) -> Result<String, String> {
        let Some(profile) = self.selected_fixture() else {
            return Err("No fixture profile selected".to_string());
        };

        let value = fixture_profile_to_value(profile);
        let yaml_value: serde_yaml::Value =
            serde_json::from_value(value).map_err(|_| "Export failed: unable to serialize profile".to_string())?;
        serde_yaml::to_string(&yaml_value)
            .map_err(|_| "Export failed: unable to write YAML".to_string())
    }

    pub fn add_process_profile(&mut self, name: &str) {
        let unique_name = self.unique_process_profile_name(name, None);
        let id = Uuid::now_v7().to_string();
        let mut defaults = schema_defaults_from_text(PROCESSING_SCHEMA_TEXT);
        if let Some(obj) = defaults.as_object_mut() {
            obj.insert("id".to_string(), Value::String(id.clone()));
            obj.insert("name".to_string(), Value::String(unique_name));
        }

        let Some(profile) = process_profile_from_value(&defaults) else {
            return;
        };
        self.process_profiles.push(profile);
        self.select_process_profile_by_id(Some(id.clone()));
        self.last_edited_process_profile_id = Some(id);
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
    }

    pub fn impact_delete_cnc_profile(&self, cnc_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(cnc) = self.machines.iter().find(|machine| machine.id == cnc_id) {
            impact.primary_profiles.push(format!("CNC profile: {}", cnc.name));
        }

        let dependent_ids: BTreeSet<String> = self
            .process_profiles
            .iter()
            .filter(|profile| profile.cnc_profile_id == cnc_id)
            .map(|profile| profile.id.clone())
            .collect();

        for profile in self
            .process_profiles
            .iter()
            .filter(|profile| dependent_ids.contains(&profile.id))
        {
            impact
                .dependent_process_profiles
                .push(format!("Machining profile: {}", profile.name));
        }

        if self
            .selected_process_profile_id
            .as_ref()
            .map(|id| dependent_ids.contains(id))
            .unwrap_or(false)
        {
            impact.deleted_live_projects.push("Active job session".to_string());
        }

        impact
    }

    pub fn impact_delete_fixture_profile(&self, fixture_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(fixture) = self.fixtures.iter().find(|item| item.id == fixture_id) {
            impact
                .primary_profiles
                .push(format!("Fixture profile: {}", fixture.name));
        }

        let dependent_ids: BTreeSet<String> = self
            .process_profiles
            .iter()
            .filter(|profile| profile.fixture_profile_id == fixture_id)
            .map(|profile| profile.id.clone())
            .collect();

        for profile in self
            .process_profiles
            .iter()
            .filter(|profile| dependent_ids.contains(&profile.id))
        {
            impact
                .dependent_process_profiles
                .push(format!("Machining profile: {}", profile.name));
        }

        if self
            .selected_process_profile_id
            .as_ref()
            .map(|id| dependent_ids.contains(id))
            .unwrap_or(false)
        {
            impact.deleted_live_projects.push("Active job session".to_string());
        }

        impact
    }

    pub fn impact_delete_process_profile(&self, process_profile_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(profile) = self
            .process_profiles
            .iter()
            .find(|profile| profile.id == process_profile_id)
        {
            impact
                .primary_profiles
                .push(format!("Machining profile: {}", profile.name));
        }
        if self
            .selected_process_profile_id
            .as_deref()
            .map(|id| id == process_profile_id)
            .unwrap_or(false)
        {
            impact.deleted_live_projects.push("Active job session".to_string());
        }
        impact
    }

    pub fn impact_delete_toolset_profile(&self, toolset_id: &str) -> CascadeDeleteImpact {
        let mut impact = CascadeDeleteImpact::default();
        if let Some(toolset) = self.toolsets.iter().find(|item| item.id == toolset_id) {
            impact
                .primary_profiles
                .push(format!("Toolset profile: {}", toolset.name));
        }

        let dependent_ids: BTreeSet<String> = self
            .process_profiles
            .iter()
            .filter(|profile| profile.toolset_profile_id == toolset_id)
            .map(|profile| profile.id.clone())
            .collect();

        for profile in self
            .process_profiles
            .iter()
            .filter(|profile| dependent_ids.contains(&profile.id))
        {
            impact
                .dependent_process_profiles
                .push(format!("Machining profile: {}", profile.name));
        }

        if self
            .selected_process_profile_id
            .as_ref()
            .map(|id| dependent_ids.contains(id))
            .unwrap_or(false)
        {
            impact.deleted_live_projects.push("Active job session".to_string());
        }

        impact
    }

    pub fn delete_cnc_profile_with_cascade(&mut self, cnc_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_cnc_profile(cnc_id);

        if !impact.dependent_process_profiles.is_empty() {
            return impact;
        }

        self.machines.retain(|machine| machine.id != cnc_id);
        self.machine_mru.retain(|id| id != cnc_id);

        let next_processing_id = self
            .selected_process_profile_id
            .clone()
            .filter(|id| self.process_profiles.iter().any(|profile| &profile.id == id))
            .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));

        if let Some(processing_id) = next_processing_id {
            self.select_process_profile_by_id(Some(processing_id));
        }

        if self.selected_process_profile_id.is_none() {
            let next_selected = self
                .machine_mru
                .iter()
                .find(|id| self.machines.iter().any(|machine| &machine.id == *id))
                .cloned()
                .or_else(|| self.machines.first().map(|machine| machine.id.clone()));

            self.select_machine_profile_by_id(next_selected);
        }

        if self.machines.is_empty() {
            self.show_first_launch = true;
            self.selected_screen = Screen::CncProfiles;
        }

        self.persist_realms(&[
            PersistRealm::CncProfiles,
            PersistRealm::GlobalSettings,
            PersistRealm::ProcessingProfiles,
        ]);

        impact
    }

    pub fn delete_fixture_profile_with_cascade(&mut self, fixture_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_fixture_profile(fixture_id);

        if !impact.dependent_process_profiles.is_empty() {
            return impact;
        }

        self.fixtures.retain(|fixture| fixture.id != fixture_id);

        let next_processing_id = self
            .selected_process_profile_id
            .clone()
            .filter(|id| self.process_profiles.iter().any(|profile| &profile.id == id))
            .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));

        self.select_process_profile_by_id(next_processing_id);

        if self
            .selected_fixture_id
            .as_ref()
            .map(|id| !self.fixtures.iter().any(|fixture| &fixture.id == id))
            .unwrap_or(false)
        {
            self.selected_fixture_id = self.fixtures.first().map(|fixture| fixture.id.clone());
        }

        self.persist_realms(&[PersistRealm::FixtureProfiles]);

        impact
    }

    pub fn delete_process_profile_with_cascade(&mut self, process_profile_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_process_profile(process_profile_id);
        self.process_profiles.retain(|profile| profile.id != process_profile_id);
        let next_processing_id = self
            .selected_process_profile_id
            .clone()
            .filter(|id| self.process_profiles.iter().any(|profile| &profile.id == id))
            .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));

        self.select_process_profile_by_id(next_processing_id);
        self.persist_realms(&[PersistRealm::ProcessingProfiles]);
        impact
    }

    pub fn delete_toolset_profile_with_cascade(&mut self, toolset_id: &str) -> CascadeDeleteImpact {
        let impact = self.impact_delete_toolset_profile(toolset_id);

        self.toolsets.retain(|toolset| toolset.id != toolset_id);

        for profile in self.process_profiles.iter_mut() {
            if profile.toolset_profile_id == toolset_id {
                // Keep active/default reference as broken so users can repair explicitly.
                profile.toolset_profile_choices = vec![toolset_id.to_string()];
            } else {
                // Clean non-active references from the allowed set.
                profile
                    .toolset_profile_choices
                    .retain(|id| id != toolset_id);
            }

            if profile.toolset_profile_id.trim().is_empty() {
                profile.pending_required_fields.insert("toolset.default".to_string());
            } else {
                profile.pending_required_fields.remove("toolset.default");
            }
            if profile.toolset_profile_choices.is_empty() {
                profile.pending_required_fields.insert("toolset.choices".to_string());
            } else {
                profile.pending_required_fields.remove("toolset.choices");
            }
            profile.usable = profile.pending_required_fields.is_empty();
        }

        let next_processing_id = self
            .selected_process_profile_id
            .clone()
            .filter(|id| self.process_profiles.iter().any(|profile| &profile.id == id))
            .or_else(|| self.process_profiles.first().map(|profile| profile.id.clone()));

        self.select_process_profile_by_id(next_processing_id);

        if self.selected_toolset_id.as_deref() == Some(toolset_id) {
            self.selected_toolset_id = Some(toolset_id.to_string());
            self.rack_slots.clear();
        }

        self.validate_current_job_references();
        self.persist_realms(&[
            PersistRealm::ToolsetProfiles,
            PersistRealm::ProcessingProfiles,
            PersistRealm::GlobalSettings,
        ]);

        impact
    }

    #[allow(dead_code)]
    pub fn add_demo_tool(&mut self) {
        let idx = self.tools.len() + 1;
        self.tools.push(Tool {
            id: self.next_tool_id(),
            composite_name: format!("0.6mm End Mill {idx}"),
            name: String::new(),
            kind: "End Mill".to_string(),
            diameter: Length::from_mm(0.6),
            catalog_diameter: None,
            point_angle: Angle::from_degrees(180.0),
            catalog_point_angle: None,
            feed_rate: None,
            catalog_feed_rate: None,
            spindle_speed: None,
            catalog_spindle_speed: None,
            status: ToolStatus::InStock,
            preference: ToolPreference::Neutral,
            source_catalog: "Manual".to_string(),
            manufacturer: None,
            sku: None,
        });
        self.persist_stock_snapshot();
    }

    fn next_tool_id(&self) -> String {
        loop {
            let candidate = Uuid::now_v7().to_string();
            if !self.tools.iter().any(|t| t.id == candidate) {
                return candidate;
            }
        }
    }

    fn unique_tool_clone_name(&self, source: &Tool) -> String {
        let base = if source.name.trim().is_empty() {
            "Copy".to_string()
        } else {
            format!("{} copy", source.name.trim())
        };

        let mut index = 1usize;
        loop {
            let candidate = if index == 1 {
                base.clone()
            } else {
                format!("{} {}", base, index)
            };
            let display_name = format!("{} - {}", source.composite_name.trim(), candidate);
            if !self
                .tools
                .iter()
                .any(|tool| tool.display_name().eq_ignore_ascii_case(&display_name))
            {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn add_tools_from_catalog_selection(&mut self, selected_tool_keys: &[String]) -> usize {
        if selected_tool_keys.is_empty() {
            return 0;
        }

        let mut added = 0usize;
        for catalog in &self.catalogs {
            for section in &catalog.sections {
                for tool in &section.tools {
                    if !selected_tool_keys.iter().any(|k| k == &tool.key) {
                        continue;
                    }

                    let has_same_sku = tool
                        .sku
                        .as_ref()
                        .map(|sku| !sku.trim().is_empty())
                        .unwrap_or(false)
                        && self
                            .tools
                            .iter()
                            .any(|existing| {
                                existing
                                    .sku
                                    .as_ref()
                                    .map(|x| x == tool.sku.as_ref().unwrap())
                                    .unwrap_or(false)
                            });
                    let has_same_identity = self
                        .tools
                        .iter()
                        .any(|existing| {
                            existing.composite_name == tool.display_name
                                && existing.kind == tool.kind
                                && (existing.diameter.as_mm() - tool.diameter.as_mm()).abs() < 0.0001
                        });
                    if has_same_sku || has_same_identity {
                        continue;
                    }

                    self.tools.push(Tool {
                        id: self.next_tool_id(),
                        composite_name: tool.display_name.clone(),
                        name: String::new(),
                        kind: tool.kind.clone(),
                        diameter: tool.diameter,
                        catalog_diameter: Some(tool.diameter),
                        point_angle: tool.point_angle,
                        catalog_point_angle: Some(tool.point_angle),
                        feed_rate: tool.feed_rate,
                        catalog_feed_rate: tool.feed_rate,
                        spindle_speed: tool.spindle_speed,
                        catalog_spindle_speed: tool.spindle_speed,
                        status: ToolStatus::InStock,
                        preference: ToolPreference::Neutral,
                        source_catalog: format!("{} / {}", catalog.name, section.name),
                        manufacturer: Some(format!("{} / {}", catalog.name, section.name)),
                        sku: tool.sku.clone(),
                    });
                    added += 1;
                }
            }
        }

        if added > 0 {
            self.persist_stock_snapshot();
        }

        added
    }

    pub fn clone_tool(&mut self, tool_id: &str) -> Option<String> {
        let source = self.tools.iter().find(|tool| tool.id == tool_id).cloned()?;
        let new_id = self.next_tool_id();
        let clone = Tool {
            id: new_id.clone(),
            name: self.unique_tool_clone_name(&source),
            ..source
        };
        self.tools.push(clone);
        self.persist_stock_snapshot();
        Some(new_id)
    }

    pub fn remove_tools(&mut self, tool_ids: &[String]) -> usize {
        if tool_ids.is_empty() {
            return 0;
        }

        let to_remove: BTreeSet<&str> = tool_ids.iter().map(|tool_id| tool_id.as_str()).collect();
        let before = self.tools.len();

        self.tools.retain(|tool| !to_remove.contains(tool.id.as_str()));

        let removed = before.saturating_sub(self.tools.len());
        if removed > 0 {
            self.persist_stock_snapshot();
            self.validate_current_job_references();
        }
        removed
    }

    pub fn toolset_referencing_process_profiles(&self, toolset_id: &str) -> Vec<String> {
        self.process_profiles
            .iter()
            .filter(|profile| {
                profile.toolset_profile_id == toolset_id
                    || profile
                        .toolset_profile_choices
                        .iter()
                        .any(|choice| choice == toolset_id)
            })
            .map(|profile| profile.name.clone())
            .collect()
    }

    pub fn select_screen(&mut self, screen: Screen) {
        self.selected_screen = screen;
    }

    #[allow(dead_code)]
    pub fn set_rotation_angle(&mut self, angle: i32) {
        self.project_config.rotation_angle = angle;
        self.gcode_modified = false;
    }

    pub fn seed_rack_slots(&mut self, slot_count: u8) {
        for slot in 1..=slot_count {
            self.rack_slots.entry(slot).or_insert(RackSlot {
                tool_id: None,
                locked: false,
                disabled: false,
            });
        }
    }
}

fn load_tools_direct_from_disk() -> Option<Vec<Tool>> {
    let app_dirs = ensure_app_dirs().ok()?;
    let raw = fs::read_to_string(&app_dirs.stock).ok()?;
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    let json_value: Value = serde_json::to_value(yaml_value).ok()?;
    Some(tools_from_stock_value(&json_value))
}

// -----------------------------------------------------------------------------
// 6) SCHEMA CONVERSION HELPERS
// -----------------------------------------------------------------------------
// Conversion helpers isolate schema document shapes from in-memory structs.
//
// Grouped by schema domain:
// - cnc.yaml         : machine_profile_to_value / machine_profile_from_value
// - fixture.yaml     : fixture_profile_to_value / fixture_profile_from_value
// - processing.yaml  : process_profile_to_value / process_profile_from_value
// - stock.yaml       : stock_value_from_tools / tools_from_stock_value
// - toolset.yaml     : toolset_profile_to_value / toolset_profile_from_value
fn machine_profile_to_value(machine: &MachineProfile) -> Value {
    json!({
        "schema_version": 1,
        "id": machine.id,
        "machine": {
            "max_feed_rate": machine.max_feed_rate.to_string(),
            "spindle_rpm_min": machine.spindle_rpm_min.to_string(),
            "spindle_rpm_max": machine.spindle_rpm_max.to_string(),
            "atc_slot_count": machine.atc_slot_count,
            "scaling": {
                "x": machine.scaling_x,
                "y": machine.scaling_y,
            },
            "line_numbering_increment": machine.line_numbering_increment,
        },
        "primitives": {
            "use_metric": "{set_precision(3)}G21",
            "use_imperial": "{set_precision(5)}G20",
            "initialise": machine.gcode_header,
            "rapid_move": machine.drill_first_move,
            "linear_cut": machine.drill_cycle_mode_series,
            "start_spindle": machine.drill_cycle_start,
            "stop_spindle": machine.drill_cycle_cancel,
            "drill": machine.drill_next_hole,
            "peck_drill": machine.drill_cycle_mode_last,
            "cut_arc": machine.route_plunge_and_offset,
            "cut_bezier": machine.route_arc_up,
            "change_tool": machine.tool_change_command,
            "conclude": machine.gcode_footer,
            "pause": machine.route_arc_down,
            "banner": machine.route_retract,
        }
    })
}

const CNC_SCHEMA_TEXT: &str = include_str!("../../resources/schemas/cnc.yaml");
const FIXTURE_SCHEMA_TEXT: &str = include_str!("../../resources/schemas/fixture.yaml");
const PROCESSING_SCHEMA_TEXT: &str = include_str!("../../resources/schemas/processing.yaml");
const TOOLSET_SCHEMA_TEXT: &str = include_str!("../../resources/schemas/toolset.yaml");

fn schema_defaults_from_text(schema_text: &str) -> Value {
    let yaml_schema: serde_yaml::Value = serde_yaml::from_str(schema_text).unwrap_or(serde_yaml::Value::Null);
    let json_schema: Value = serde_json::to_value(yaml_schema).unwrap_or(Value::Null);
    crate::config::defaults::populate_defaults(&json_schema).unwrap_or_else(|| json!({}))
}

fn has_path(value: &Value, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let pointer = format!("/{}", path.replace('.', "/"));
    value.pointer(&pointer).is_some()
}

fn machine_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "machine.max_feed_rate",
        "machine.spindle_rpm_min",
        "machine.spindle_rpm_max",
        "machine.atc_slot_count",
        "machine.scaling.x",
        "machine.scaling.y",
        "machine.line_numbering_increment",
        "primitives.initialise",
        "primitives.rapid_move",
        "primitives.linear_cut",
        "primitives.start_spindle",
        "primitives.stop_spindle",
        "primitives.drill",
        "primitives.peck_drill",
        "primitives.cut_arc",
        "primitives.cut_bezier",
        "primitives.change_tool",
        "primitives.conclude",
    ]
}

fn fixture_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "name",
        "board_holding_method",
        "work_origin_reference",
    ]
}

fn process_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "name",
        "cnc.default",
        "cnc.choices",
        "fixture.default",
        "fixture.choices",
        "toolset.default",
        "toolset.choices",
        "operations",
    ]
}

fn toolset_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "name",
        "generation_policy",
        "slots",
    ]
}

fn collect_missing_required(value: &Value, required_paths: &[&str]) -> BTreeSet<String> {
    required_paths
        .iter()
        .filter(|path| !has_path(value, path))
        .map(|path| (*path).to_string())
        .collect()
}

fn is_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn machine_profile_from_value(value: &Value) -> Option<MachineProfile> {
    let pending_required_fields = collect_missing_required(value, machine_required_paths());

    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(id) = id else {
        warn!("Skipping CNC profile: missing id");
        return None;
    };
    if !is_uuid(&id) {
        warn!("Skipping CNC profile '{}': id is not a UUID", id);
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| "Unnamed CNC profile".to_string());

    let max_feed_rate = value
        .pointer("/machine/max_feed_rate")
        .and_then(Value::as_str)
        .and_then(|raw| units::FeedRate::from_string(raw, Some(units::FeedRateUnit::MmPerMin)).ok())
        .or_else(|| value.get("max_feed_rate_mm_per_min").and_then(Value::as_u64).map(|v| units::FeedRate::from_mm_per_min(v as f64)))
        .unwrap_or_else(|| units::FeedRate::from_mm_per_min(2000.0));

    let spindle_rpm_min = value
        .pointer("/machine/spindle_rpm_min")
        .and_then(Value::as_str)
        .and_then(|raw| units::RotationalSpeed::from_string(raw, Some(units::RotationalSpeedUnit::Rpm)).ok())
        .or_else(|| value.get("spindle_min_rpm").and_then(Value::as_u64).map(|v| units::RotationalSpeed::from_rpm(v as f64)))
        .unwrap_or_else(|| units::RotationalSpeed::from_rpm(3000.0));

    let spindle_rpm_max = value
        .pointer("/machine/spindle_rpm_max")
        .and_then(Value::as_str)
        .and_then(|raw| units::RotationalSpeed::from_string(raw, Some(units::RotationalSpeedUnit::Rpm)).ok())
        .or_else(|| value.get("spindle_max_rpm").and_then(Value::as_u64).map(|v| units::RotationalSpeed::from_rpm(v as f64)))
        .unwrap_or_else(|| units::RotationalSpeed::from_rpm(24000.0));

    let atc_slot_count = value
        .pointer("/machine/atc_slot_count")
        .and_then(Value::as_u64)
        .map(|v| v as u8)
        .or_else(|| value.get("atc_slot_count").and_then(Value::as_u64).map(|v| v as u8))
        .unwrap_or(0);

    let scaling_x = value
        .pointer("/machine/scaling/x")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .or_else(|| value.get("scaling_x").and_then(Value::as_f64).map(|v| v as f32))
        .unwrap_or(100.0);

    let scaling_y = value
        .pointer("/machine/scaling/y")
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .or_else(|| value.get("scaling_y").and_then(Value::as_f64).map(|v| v as f32))
        .unwrap_or(100.0);

    Some(MachineProfile {
        id,
        name,
        built_in: false,
        max_feed_rate,
        spindle_rpm_min,
        spindle_rpm_max,
        atc_slot_count,
        scaling_x,
        scaling_y,
        line_numbering_increment: value
            .pointer("/machine/line_numbering_increment")
            .and_then(Value::as_u64)
            .map(|v| v as u16)
            .or_else(|| value.get("line_numbering_increment").and_then(Value::as_u64).map(|v| v as u16))
            .unwrap_or(10),
        gcode_header: value
            .pointer("/primitives/initialise")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/gcode_header").and_then(Value::as_str))
            .or_else(|| value.get("header").and_then(Value::as_str))
            .or_else(|| value.get("gcode_header").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        gcode_footer: value
            .pointer("/primitives/conclude")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/gcode_footer").and_then(Value::as_str))
            .or_else(|| value.get("footer").and_then(Value::as_str))
            .or_else(|| value.get("gcode_footer").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_first_move: value
            .pointer("/primitives/rapid_move")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_first_move").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/first_move").and_then(Value::as_str))
            .or_else(|| value.get("drill_first_move").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_cycle_mode_last: value
            .pointer("/primitives/peck_drill")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_cycle_mode_last").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/cycle_mode_last").and_then(Value::as_str))
            .or_else(|| value.get("drill_cycle_mode_last").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_cycle_mode_series: value
            .pointer("/primitives/linear_cut")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_cycle_mode_series").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/cycle_mode_series").and_then(Value::as_str))
            .or_else(|| value.get("drill_cycle_mode_series").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_cycle_start: value
            .pointer("/primitives/start_spindle")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_cycle_start").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/cycle_start").and_then(Value::as_str))
            .or_else(|| value.get("drill_cycle_start").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_next_hole: value
            .pointer("/primitives/drill")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_next_hole").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/next_hole").and_then(Value::as_str))
            .or_else(|| value.get("drill_next_hole").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        drill_cycle_cancel: value
            .pointer("/primitives/stop_spindle")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/drill_cycle_cancel").and_then(Value::as_str))
            .or_else(|| value.pointer("/drill/cycle_cancel").and_then(Value::as_str))
            .or_else(|| value.get("drill_cycle_cancel").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        route_plunge_and_offset: value
            .pointer("/primitives/cut_arc")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/route_plunge_and_offset").and_then(Value::as_str))
            .or_else(|| value.pointer("/route/plunge_and_offset").and_then(Value::as_str))
            .or_else(|| value.get("route_plunge_and_offset").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        route_arc_up: value
            .pointer("/primitives/cut_bezier")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/route_arc_up").and_then(Value::as_str))
            .or_else(|| value.pointer("/route/arc_up").and_then(Value::as_str))
            .or_else(|| value.get("route_arc_up").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        route_arc_down: value
            .pointer("/primitives/pause")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/route_arc_down").and_then(Value::as_str))
            .or_else(|| value.pointer("/route/arc_down").and_then(Value::as_str))
            .or_else(|| value.get("route_arc_down").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        route_retract: value
            .pointer("/primitives/banner")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/route_retract").and_then(Value::as_str))
            .or_else(|| value.pointer("/route/retract").and_then(Value::as_str))
            .or_else(|| value.get("route_retract").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        tool_change_command: value
            .pointer("/primitives/change_tool")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/templates/tool_change_command").and_then(Value::as_str))
            .or_else(|| value.pointer("/tool_change/command").and_then(Value::as_str))
            .or_else(|| value.get("tool_change_command").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

fn fixture_profile_to_value(fixture: &FixtureProfile) -> Value {
    json!({
        "schema_version": 1,
        "id": fixture.id,
        "name": fixture.name,
        "board_holding_method": fixture.backing_board,
        "work_origin_reference": {
            "x0": "Left",
            "y0": "Front",
            "z0_reference": fixture.coordinate_context,
        },
        "backboard_thickness": "2.5mm",
    })
}

fn fixture_profile_from_value(value: &Value) -> Option<FixtureProfile> {
    let pending_required_fields = collect_missing_required(value, fixture_required_paths());
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(id) = id else {
        warn!("Skipping fixture profile: missing id");
        return None;
    };
    if !is_uuid(&id) {
        warn!("Skipping fixture profile '{}': id is not a UUID", id);
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| "Unnamed fixture profile".to_string());

    Some(FixtureProfile {
        id,
        name,
        coordinate_context: value
            .pointer("/work_origin_reference/z0_reference")
            .and_then(Value::as_str)
            .or_else(|| value.get("coordinate_context").and_then(Value::as_str))
            .unwrap_or("Fixture-defined board origin")
            .to_string(),
        backing_board: value
            .get("board_holding_method")
            .and_then(Value::as_str)
            .or_else(|| value.get("backing_board").and_then(Value::as_str))
            .unwrap_or("MDF spoilboard")
            .to_string(),
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

fn process_profile_to_value(profile: &JobProfile) -> Value {
    let cnc_choices = if profile.cnc_profile_choices.is_empty() {
        vec![profile.cnc_profile_id.clone()]
    } else {
        profile.cnc_profile_choices.clone()
    };
    let fixture_choices = if profile.fixture_profile_choices.is_empty() {
        vec![profile.fixture_profile_id.clone()]
    } else {
        profile.fixture_profile_choices.clone()
    };
    let toolset_choices = if profile.toolset_profile_choices.is_empty() {
        vec![profile.toolset_profile_id.clone()]
    } else {
        profile.toolset_profile_choices.clone()
    };

    let mut value = json!({
        "schema_version": 2,
        "id": profile.id,
        "name": profile.name,
        "side_to_machine": profile.side.as_str(),
        "cnc": {
            "default": profile.cnc_profile_id,
            "choices": cnc_choices,
        },
        "fixture": {
            "default": profile.fixture_profile_id,
            "choices": fixture_choices,
        },
        "toolset": {
            "default": profile.toolset_profile_id,
            "choices": toolset_choices,
        },
        "operations": profile
            .default_operations
            .iter()
            .map(|op| operation_to_key(*op))
            .collect::<Vec<_>>(),
        "routing": {
            "cut_depth_strategy": profile.cut_depth_strategy.as_str(),
            "multi_pass_max_depth": profile.multi_pass_max_depth.to_string(),
        },
    });

    if let Some(root) = value.as_object_mut() {
        for op in ProductionOperation::all().iter().copied() {
            let key = operation_to_key(op);
            let op_value = profile
                .operation_setups
                .get(key)
                .cloned()
                .unwrap_or_else(|| default_operation_setup_value(op));
            root.insert(key.to_string(), op_value);
        }
    }

    value
}

fn process_profile_from_value(value: &Value) -> Option<JobProfile> {
    let mut pending_required_fields = collect_missing_required(value, process_required_paths());

    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(id) = id else {
        warn!("Skipping machining profile: missing id");
        return None;
    };
    if !is_uuid(&id) {
        warn!("Skipping machining profile '{}': id is not a UUID", id);
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| "Unnamed machining profile".to_string());

    let side = match value
        .get("side_to_machine")
        .and_then(Value::as_str)
        .unwrap_or("top")
    {
        "bottom" => Side::Bottom,
        _ => Side::Top,
    };

    let cut_depth_strategy = match value
        .pointer("/routing/cut_depth_strategy")
        .and_then(Value::as_str)
        .unwrap_or("automatic")
    {
        "single_pass" => CutDepthStrategy::SinglePass,
        "multi_pass" => CutDepthStrategy::MultiPass,
        _ => CutDepthStrategy::Automatic,
    };
    let multi_pass_max_depth = value
        .pointer("/routing/multi_pass_max_depth")
        .and_then(Value::as_str)
        .and_then(|raw| units::Length::from_string(raw, Some(units::LengthUnit::Mm)).ok())
        .or_else(|| value.pointer("/routing/multi_pass_max_depth").and_then(value_to_length_mm).map(units::Length::from_mm))
        .unwrap_or_else(|| units::Length::from_mm(1.0));

    let cnc_profile_id = value
        .pointer("/cnc/default")
        .and_then(Value::as_str)
        .or_else(|| value.get("cnc_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    let fixture_profile_id = value
        .pointer("/fixture/default")
        .and_then(Value::as_str)
        .or_else(|| value.get("fixture_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    let toolset_profile_id = value
        .pointer("/toolset/default")
        .and_then(Value::as_str)
        .or_else(|| value.get("toolset_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    let mut cnc_profile_choices = extract_binding_choices(value, "cnc", &cnc_profile_id);
    let mut fixture_profile_choices = extract_binding_choices(value, "fixture", &fixture_profile_id);
    let mut toolset_profile_choices = extract_binding_choices(value, "toolset", &toolset_profile_id);

    let mut default_operations = value
        .get("operations")
        .and_then(Value::as_array)
        .map(|ops| {
            ops.iter()
                .filter_map(Value::as_str)
                .filter_map(operation_from_key)
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            value
                .get("default_operations")
                .and_then(Value::as_array)
                .map(|ops| {
                    ops.iter()
                        .filter_map(Value::as_str)
                        .filter_map(operation_from_key)
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_default();

    if default_operations.is_empty() {
        default_operations = ProductionOperation::all()
            .into_iter()
            .filter(|op| {
                value
                    .pointer(&format!("/{}/enabled", operation_to_key(*op)))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .collect();
    }

    let mut operation_setups = extract_operation_setups(value);
    for op in ProductionOperation::all().iter().copied() {
        let key = operation_to_key(op).to_string();
        let default_setup = default_operation_setup_value(op);
        let setup = operation_setups
            .entry(key)
            .or_insert_with(|| default_setup.clone());
        merge_object_defaults(setup, &default_setup);
        if let Some(obj) = setup.as_object_mut() {
            obj.insert(
                "enabled".to_string(),
                Value::Bool(default_operations.contains(&op)),
            );
        }
    }

    if cnc_profile_id.trim().is_empty() {
        pending_required_fields.insert("cnc.default".to_string());
        pending_required_fields.insert("cnc.choices".to_string());
    } else if !is_uuid(&cnc_profile_id) {
        warn!(
            "Skipping machining profile '{}': cnc.default is not a UUID ({})",
            id,
            cnc_profile_id
        );
        return None;
    } else {
        if !cnc_profile_choices.iter().any(|existing| existing == &cnc_profile_id) {
            cnc_profile_choices.push(cnc_profile_id.clone());
        }
        sort_uuid_v7_ids(&mut cnc_profile_choices);
        if cnc_profile_choices.is_empty() {
            pending_required_fields.insert("cnc.choices".to_string());
        } else {
            pending_required_fields.remove("cnc.choices");
        }
    }
    if fixture_profile_id.trim().is_empty() {
        pending_required_fields.insert("fixture.default".to_string());
        pending_required_fields.insert("fixture.choices".to_string());
    } else if !is_uuid(&fixture_profile_id) {
        warn!(
            "Skipping machining profile '{}': fixture.default is not a UUID ({})",
            id,
            fixture_profile_id
        );
        return None;
    } else {
        if !fixture_profile_choices
            .iter()
            .any(|existing| existing == &fixture_profile_id)
        {
            fixture_profile_choices.push(fixture_profile_id.clone());
        }
        sort_uuid_v7_ids(&mut fixture_profile_choices);
        if fixture_profile_choices.is_empty() {
            pending_required_fields.insert("fixture.choices".to_string());
        } else {
            pending_required_fields.remove("fixture.choices");
        }
    }
    if toolset_profile_id.trim().is_empty() {
        pending_required_fields.insert("toolset.default".to_string());
        pending_required_fields.insert("toolset.choices".to_string());
    } else if !is_uuid(&toolset_profile_id) {
        warn!(
            "Skipping machining profile '{}': toolset.default is not a UUID ({})",
            id,
            toolset_profile_id
        );
        return None;
    } else {
        if !toolset_profile_choices
            .iter()
            .any(|existing| existing == &toolset_profile_id)
        {
            toolset_profile_choices.push(toolset_profile_id.clone());
        }
        sort_uuid_v7_ids(&mut toolset_profile_choices);
        if toolset_profile_choices.is_empty() {
            pending_required_fields.insert("toolset.choices".to_string());
        } else {
            pending_required_fields.remove("toolset.choices");
        }
    }
    if default_operations.is_empty() {
        pending_required_fields.insert("operations".to_string());
    }

    Some(JobProfile {
        id,
        name,
        cnc_profile_id,
        cnc_profile_choices,
        fixture_profile_id,
        fixture_profile_choices,
        toolset_profile_id,
        toolset_profile_choices,
        side,
        default_operations,
        cut_depth_strategy,
        multi_pass_max_depth,
        operation_setups,
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

fn extract_operation_setups(value: &Value) -> BTreeMap<String, Value> {
    let mut setups = BTreeMap::new();
    for op in ProductionOperation::all().iter().copied() {
        let key = operation_to_key(op);
        if let Some(v) = value.get(key) {
            setups.insert(key.to_string(), v.clone());
        }
    }
    setups
}

fn default_operation_setup_value(op: ProductionOperation) -> Value {
    match op {
        ProductionOperation::DrillLocatingPins => json!({
            "enabled": false,
        }),
        ProductionOperation::DrillPth | ProductionOperation::DrillNpth => json!({
            "enabled": false,
            "holes": {
                "oversize": {
                    "relative": "8%",
                    "max": "0.20mm",
                },
                "undersize": {
                    "relative": "8%",
                    "max": "0.20mm",
                },
                "route_fallback": false,
                "drill_first": true,
                "pilot": false,
                "oblong": "drill_ends_then_route",
            }
        }),
        ProductionOperation::RouteBoard => json!({
            "enabled": false,
            "edge": {
                "cut": "route",
                "retention": "tabs",
                "tabs": 4,
                "tab_width": "2.0mm",
                "bite_holes": 3,
                "vgroove_depth": "80%",
            },
            "finishing": {
                "clearance": "0.1mm",
                "direction": "climb",
            }
        }),
        ProductionOperation::MillBoard => json!({
            "enabled": false,
            "finishing": {
                "clearance": "0.1mm",
                "direction": "climb",
            }
        }),
    }
}

fn set_nested_value(root: &mut Value, path: &[&str], value: Value) {
    if path.is_empty() {
        return;
    }

    if !root.is_object() {
        *root = json!({});
    }

    let mut current = root;
    for key in &path[..path.len() - 1] {
        if !current.is_object() {
            *current = json!({});
        }
        let obj = current.as_object_mut().expect("object expected");
        current = obj
            .entry((*key).to_string())
            .or_insert_with(|| json!({}));
    }

    if let Some(obj) = current.as_object_mut() {
        obj.insert(path[path.len() - 1].to_string(), value);
    }
}

fn merge_object_defaults(target: &mut Value, defaults: &Value) {
    let Some(default_obj) = defaults.as_object() else {
        return;
    };

    if !target.is_object() {
        *target = json!({});
    }

    let Some(target_obj) = target.as_object_mut() else {
        return;
    };

    for (key, default_value) in default_obj {
        if let Some(existing) = target_obj.get_mut(key) {
            if existing.is_object() && default_value.is_object() {
                merge_object_defaults(existing, default_value);
            }
        } else {
            target_obj.insert(key.clone(), default_value.clone());
        }
    }
}

fn extract_binding_choices(value: &Value, domain: &str, default_id: &str) -> Vec<String> {
    let mut choices = value
        .pointer(&format!("/{domain}/choices"))
        .and_then(|v| {
            if let Some(arr) = v.as_array() {
                Some(
                    arr.iter()
                        .filter_map(Value::as_str)
                        .filter(|candidate| is_uuid(candidate))
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                )
            } else if v.as_str() == Some("any") {
                Some(Vec::new())
            } else {
                None
            }
        })
        .unwrap_or_default();

    if !default_id.trim().is_empty() && !choices.iter().any(|existing| existing == default_id) {
        choices.push(default_id.to_string());
    }
    sort_uuid_v7_ids(&mut choices);
    choices
}

fn sort_uuid_v7_ids(ids: &mut Vec<String>) {
    ids.sort();
    ids.dedup();
}

fn normalize_binding_choices(choices: &[String], default_id: &str) -> Vec<String> {
    let mut normalized = choices
        .iter()
        .filter(|id| is_uuid(id))
        .cloned()
        .collect::<Vec<_>>();

    if is_uuid(default_id) && !normalized.iter().any(|id| id == default_id) {
        normalized.push(default_id.to_string());
    }

    sort_uuid_v7_ids(&mut normalized);
    normalized
}

// toolset.yaml -> ToolsetProfile conversion boundary.
fn toolset_profile_from_value(value: &Value) -> Option<ToolsetProfile> {
    let mut pending_required_fields = collect_missing_required(value, toolset_required_paths());

    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let Some(id) = id else {
        warn!("Skipping toolset profile: missing id");
        return None;
    };
    if !is_uuid(&id) {
        warn!("Skipping toolset profile '{}': id is not a UUID", id);
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| "Unnamed toolset profile".to_string());
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let generation_policy = ToolsetGenerationPolicy::from_key(
        value
            .get("generation_policy")
            .and_then(Value::as_str)
            .unwrap_or("allow_hybrid"),
    );

    let mut slots = BTreeMap::new();
    for slot in value
        .get("slots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(index) = slot.get("index").and_then(Value::as_u64).map(|v| v as u8) else {
            continue;
        };
        let mode = slot.get("mode").and_then(Value::as_str).unwrap_or("spare");
        let tool_id = slot
            .get("tool_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if let Some(tool_id) = tool_id.as_ref() {
            if !is_uuid(tool_id) {
                warn!(
                    "Skipping toolset profile '{}': slot tool_id is not a UUID ({})",
                    id,
                    tool_id
                );
                return None;
            }
        }
        slots.insert(
            index,
            RackSlot {
                tool_id,
                locked: slot.get("locked").and_then(Value::as_bool).unwrap_or(mode == "fixed"),
                disabled: slot
                    .get("disabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(mode == "do_not_use"),
            },
        );
    }

    if slots.is_empty() {
        pending_required_fields.insert("slots".to_string());
    }

    Some(ToolsetProfile {
        id,
        name,
        description,
        generation_policy,
        slots,
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

// ToolsetProfile -> toolset.yaml conversion boundary.
fn toolset_profile_to_value(profile: &ToolsetProfile) -> Value {
    let slot_values = profile
        .slots
        .iter()
        .map(|(index, slot)| {
            let mode = if slot.disabled {
                "do_not_use"
            } else if slot.locked {
                "fixed"
            } else {
                "spare"
            };

            let mut value = json!({
                "index": index,
                "mode": mode,
            });

            if let Some(tool_id) = &slot.tool_id {
                value["tool_id"] = Value::String(tool_id.clone());
            }

            value
        })
        .collect::<Vec<_>>();

    json!({
        "schema_version": 1,
        "id": profile.id,
        "name": profile.name,
        "description": profile.description,
        "generation_policy": profile.generation_policy.as_key(),
        "slots": slot_values,
    })
}

fn build_toolset_profiles(toolsets: &[ToolsetProfile]) -> BTreeMap<String, Value> {
    toolsets
        .iter()
        .map(|profile| (profile.id.clone(), toolset_profile_to_value(profile)))
        .collect()
}

fn operation_to_key(operation: ProductionOperation) -> &'static str {
    match operation {
        ProductionOperation::DrillLocatingPins => "drill_locating_pins",
        ProductionOperation::DrillPth => "drill_pth",
        ProductionOperation::DrillNpth => "drill_npth",
        ProductionOperation::RouteBoard => "route_board",
        ProductionOperation::MillBoard => "mill_board",
    }
}

fn operation_from_key(value: &str) -> Option<ProductionOperation> {
    match value {
        "drill_locating_pins" => Some(ProductionOperation::DrillLocatingPins),
        "drill_pth" => Some(ProductionOperation::DrillPth),
        "drill_npth" => Some(ProductionOperation::DrillNpth),
        "route_board" => Some(ProductionOperation::RouteBoard),
        "mill_board" => Some(ProductionOperation::MillBoard),
        _ => None,
    }
}

fn value_to_length(value: &Value) -> Option<Length> {
    match value {
        Value::String(v) => Length::from_string(v, None).ok(),
        Value::Number(v) => v.as_f64().map(Length::from_mm),
        _ => None,
    }
}

fn value_to_length_mm(value: &Value) -> Option<f64> {
    value_to_length(value).map(Length::as_mm)
}

fn load_persisted_unit_system() -> UnitSystem {
    let Some(state) = persistence_state() else {
        return UnitSystem::Metric;
    };

    let units_value = state
        .global_settings
        .get("units")
        .and_then(Value::as_str)
        .or_else(|| {
            // Backward compatibility for legacy nested shape.
            state
                .global_settings
                .get("units")
                .and_then(|units| units.get("system"))
                .and_then(Value::as_str)
        });

    match units_value {
        Some("mil") => UnitSystem::Mil,
        Some("in") | Some("imperial") => UnitSystem::Imperial,
        _ => UnitSystem::Metric,
    }
}

fn load_persisted_theme() -> Theme {
    let Some(state) = persistence_state() else {
        return Theme::Dark;
    };

    let theme_mode = state
        .global_settings
        .get("theme")
        .and_then(Value::as_str)
        .or_else(|| {
            // Backward compatibility for legacy nested shape.
            state
                .global_settings
                .get("theme")
                .and_then(|theme| theme.get("mode"))
                .and_then(Value::as_str)
        })
        .unwrap_or("dark");

    Theme::from_str(&theme_mode.to_ascii_lowercase())
}

pub fn sample_gcode() -> String {
    "; KiCad CNC Generator - GCode Output\n\
; Generated from Dioxus UI\n\
G21\n\
G90\n\
M3 S12000\n\
G0 Z5.0\n\
G0 X20.0 Y20.0\n\
G1 Z-1.6 F200\n\
G0 Z5.0\n\
G0 X180.0 Y130.0\n\
G1 Z-1.6 F200\n\
G0 Z5.0\n\
M5\n\
M30\n"
        .to_string()
}
