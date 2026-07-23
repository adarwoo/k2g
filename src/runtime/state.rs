struct RuntimeIssueDraft {
    domain: String,
    owner_tag: Option<String>,
    message: String,
    details: Option<String>,
}

impl AppState {
    // Creates runtime defaults, then hydrates persisted data from disk.
    pub fn new(boot: &UiLaunchData) -> Self {
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
            catalogs: vec![],
            tools,
            errors: vec![],
            events: vec![],
            generation_state: GenerationState::Idle,
            project_config: JobConfig {
                selected_operations: vec![ProductionOperation::DrillPth],
                rotation_angle: 0,
                tab_count: 4,
                tab_width: Length::from_mm(3.0),
                tab_width_baseline: Length::from_mm(3.0),
                allow_routing_holes: true,
                drill_then_route: false,
                pilot_hole_fallback: true,
                outline_router_tool_id: None,
                mouse_bites_enabled: false,
                mouse_bite_pitch: Length::from_mm(0.8),
                mouse_bite_drill_tool_id: None,
            },
            gcode: sample_gcode(),
            gcode_modified: false,
            suppress_persistence: false,
            show_first_launch: true,
            rack_slots: BTreeMap::new(),
            board: boot.board_snapshot.clone(),
            kicad_status: boot.kicad_status.clone(),
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

        // The live job's machining profile is the authoritative selection; fall
        // back to the last-edited profile when the job has none yet.
        let selected_process = persisted
            .job_machining_profile
            .clone()
            .or_else(|| persisted.last_edited_process_profile_id.clone())
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

        // Project the persisted board orientation into the live runtime config so
        // it survives a restart (the singleton `job.yaml` is the source of truth).
        self.project_config.rotation_angle = persisted.job_board_orientation;

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

        for realm in realms {
            match realm {
                PersistRealm::GlobalSettings => self.persist_global_settings(&app_dirs),
            }
        }
    }

    fn persist_global_settings(&self, _app_dirs: &AppDirs) {
        // Global settings are owned by the AppData datastore (see `crate::data`),
        // the sole writer of `global.setting.yaml`. Guarded on `appdata_ready` so
        // early/test contexts (no live store) are a no-op rather than a panic.
        if !crate::data::appdata_ready() {
            return;
        }
        let payload = self.make_global_settings_payload();
        match crate::data::with_appdata_mut(|data| data.replace_settings_from_value(&payload)) {
            Some(problems) if !problems.is_empty() => {
                log::warn!("Failed to persist global settings: {} problem(s)", problems.len());
            }
            _ => log::info!(
                "Persisted global settings: process={} cnc={} fixture={} toolset={}",
                self.selected_process_profile_id.clone().unwrap_or_default(),
                self.selected_machine_id.clone().unwrap_or_default(),
                self.selected_fixture_id.clone().unwrap_or_default(),
                self.selected_toolset_id.clone().unwrap_or_default(),
            ),
        }
    }

    fn make_global_settings_payload(&self) -> Value {
        json!({
            "schema_version": 1,
            "units": self.unit_system.as_settings_str(),
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

    fn profile_owner_tag(kind: &str, id: &str) -> String {
        format!("{kind}:{id}")
    }

    pub fn selected_machine(&self) -> Option<&MachineProfile> {
        self.selected_machine_id
            .as_ref()
            .and_then(|id| self.machines.iter().find(|m| &m.id == id))
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
        self.persist_job();
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Mirrors the active machining-profile selection into the live job singleton
    /// (`job.yaml`), so the job persists what it runs. A no-op during startup
    /// hydration or when the store is not ready.
    fn persist_job(&self) {
        if self.suppress_persistence || !crate::data::appdata_ready() {
            return;
        }
        let target = self
            .selected_process_profile_id
            .as_ref()
            .and_then(|id| Uuid::parse_str(id).ok());
        crate::data::with_appdata_mut(|data| data.set_job_machining_profile(target));
    }

    /// Updates the board orientation angle (degrees) on the live runtime config
    /// and writes it through to the job singleton (`job.yaml`) so it persists.
    /// Clamps to the schema range; a no-op write during startup hydration.
    pub fn set_board_orientation(&mut self, angle: i32) {
        let angle = angle.clamp(-180, 180);
        self.project_config.rotation_angle = angle;
        if self.suppress_persistence || !crate::data::appdata_ready() {
            return;
        }
        crate::data::with_appdata_mut(|data| data.set_job_board_orientation(angle));
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

    /// Runs the per-step tool-selection plan and raises a blocking error for any step
    /// with no solution, so the status pill and diagnostics banner reflect that the
    /// job cannot be machined until it is fixed. De-duplicates against what is already
    /// posted so re-running on every mutation does not re-toast an unchanged failure.
    pub fn validate_tooling(&mut self) {
        let failures: Vec<(String, Vec<String>)> = if crate::data::appdata_ready() {
            crate::runtime::tooling::plan_tooling(self)
                .steps
                .into_iter()
                .filter_map(|step| match step.outcome {
                    crate::runtime::tooling::StepOutcome::Failed(messages) => Some((step.name, messages)),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };

        let next: Vec<(String, Option<String>)> = failures
            .into_iter()
            .map(|(name, messages)| {
                (format!("No tooling solution for step '{name}'."), Some(messages.join("\n")))
            })
            .collect();
        let current: Vec<(String, Option<String>)> = self
            .errors
            .iter()
            .filter(|error| error.domain == "tooling")
            .map(|error| (error.message.clone(), error.details.clone()))
            .collect();
        if next == current {
            return; // unchanged — avoid re-posting and re-toasting
        }

        self.clear_runtime_errors("tooling");
        for (message, details) in next {
            self.push_runtime_error_owned("tooling", None, message, details);
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

    pub fn selected_machine_has_atc(&self) -> bool {
        self.selected_machine()
            .map(|m| m.atc_slot_count > 0)
            .unwrap_or(false)
    }

    pub fn select_machine_profile_by_id(&mut self, id: Option<String>) {
        self.selected_machine_id = id.clone();
        if let Some(id) = id {
            self.machine_mru.retain(|m| m != &id);
            self.machine_mru.insert(0, id);
        }
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Rebuilds the in-memory CNC machine list from the `AppData`-owned CNC
    /// documents (serialized to JSON values). AppData persists the CNC realm;
    /// this projection keeps the legacy consumers — the GCode generator, the
    /// setup screen list, and the active machine selection — coherent while the
    /// two layers coexist. Selection and MRU entries whose profiles no longer
    /// exist are pruned. Does not itself persist (AppData already wrote the file).
    pub fn refresh_machines(&mut self, values: &[Value]) {
        let machines: Vec<MachineProfile> =
            values.iter().filter_map(machine_profile_from_value).collect();
        if !machines.is_empty() {
            self.show_first_launch = false;
        }

        let ids: BTreeSet<String> = machines.iter().map(|m| m.id.clone()).collect();
        self.machine_mru.retain(|id| ids.contains(id));
        if let Some(selected) = self.selected_machine_id.clone() {
            if !ids.contains(&selected) {
                self.selected_machine_id = machines.first().map(|m| m.id.clone());
            }
        }

        self.machines = machines;
    }

    /// Rebuilds the in-memory machining (process) profile list from the
    /// `AppData`-owned machining documents. AppData persists that realm; this
    /// projection keeps the legacy consumers — the GCode generator and the active
    /// selection — coherent while the two layers coexist. A selection whose
    /// profile no longer exists is repointed. Does not itself persist.
    pub fn refresh_process_profiles(&mut self, values: &[Value]) {
        let profiles: Vec<JobProfile> =
            values.iter().filter_map(process_profile_from_value).collect();

        let ids: BTreeSet<String> = profiles.iter().map(|p| p.id.clone()).collect();
        if let Some(selected) = self.selected_process_profile_id.clone() {
            if !ids.contains(&selected) {
                self.selected_process_profile_id = profiles.first().map(|p| p.id.clone());
            }
        }

        self.process_profiles = profiles;
    }

    /// Rebuilds the in-memory toolset list from the `AppData`-owned toolset
    /// documents, and refreshes the active `rack_slots` from the selected toolset.
    /// AppData persists that realm; this projection keeps the legacy consumers —
    /// the GCode generator and the rack view — coherent. A selection whose toolset
    /// no longer exists is repointed. Does not itself persist.
    pub fn refresh_toolsets(&mut self, values: &[Value]) {
        let toolsets: Vec<ToolsetProfile> =
            values.iter().filter_map(toolset_profile_from_value).collect();

        let ids: BTreeSet<String> = toolsets.iter().map(|t| t.id.clone()).collect();
        if let Some(selected) = self.selected_toolset_id.clone() {
            if !ids.contains(&selected) {
                self.selected_toolset_id = toolsets.first().map(|t| t.id.clone());
            }
        }

        self.toolsets = toolsets;

        match self
            .selected_toolset_id
            .clone()
            .and_then(|sel| self.toolsets.iter().find(|t| t.id == sel))
        {
            Some(toolset) => self.rack_slots = toolset.slots.clone(),
            None => self.rack_slots.clear(),
        }
    }

    /// Rebuilds the in-memory `tools` (stock inventory) from the `AppData`-owned
    /// stock document. AppData persists the stock singleton; this projection keeps
    /// the legacy consumers — the GCode generator and the toolset rack picker —
    /// coherent. Does not itself persist.
    pub fn refresh_tools(&mut self, stock_value: &Value) {
        self.tools = tools_from_stock_value(stock_value);
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

    fn next_tool_id(&self) -> String {
        loop {
            let candidate = Uuid::now_v7().to_string();
            if !self.tools.iter().any(|t| t.id == candidate) {
                return candidate;
            }
        }
    }

    /// Builds the stock tools to add for a catalog-picker selection: resolves each
    /// selected catalog tool, skipping any already present — in stock or already
    /// queued this call — by non-empty SKU, or by (label, kind, diameter) identity.
    /// Pure: the caller projects the result to the stock document (the AppData
    /// writer). Returns the new tools in catalog order.
    pub fn build_catalog_tool_additions(&self, selected_tool_keys: &[String]) -> Vec<Tool> {
        let mut additions: Vec<Tool> = Vec::new();
        if selected_tool_keys.is_empty() {
            return additions;
        }

        for catalog in &self.catalogs {
            for section in &catalog.sections {
                for tool in &section.tools {
                    if !selected_tool_keys.iter().any(|k| k == &tool.key) {
                        continue;
                    }

                    let has_sku = tool.sku.as_ref().map(|sku| !sku.trim().is_empty()).unwrap_or(false);
                    let is_duplicate = self.tools.iter().chain(additions.iter()).any(|existing| {
                        (has_sku && existing.sku.as_deref() == tool.sku.as_deref())
                            || (existing.composite_name == tool.display_name
                                && existing.kind == tool.kind
                                && (existing.diameter.as_mm() - tool.diameter.as_mm()).abs() < 0.0001)
                    });
                    if is_duplicate {
                        continue;
                    }

                    additions.push(Tool {
                        id: self.next_tool_id(),
                        composite_name: tool.display_name.clone(),
                        name: String::new(),
                        kind: tool.kind.clone(),
                        diameter: tool.diameter,
                        catalog_diameter: Some(tool.diameter),
                        point_angle: tool.point_angle,
                        catalog_point_angle: Some(tool.point_angle),
                        flute_length: None,
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
                }
            }
        }

        additions
    }

    pub fn select_screen(&mut self, screen: Screen) {
        self.selected_screen = screen;
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

/// Produces a "flat" machining value for the current single-setup projection: the
/// top-level identity fields (id/name/schema_version) plus every field of the
/// first step lifted to the top level. If the value has no `steps` (already flat,
/// e.g. a hand-built fingerprint value), it is returned unchanged.
fn flatten_first_step(value: &Value) -> Value {
    let Some(step) = value.pointer("/steps/0").and_then(Value::as_object) else {
        return value.clone();
    };
    let mut flat = step.clone();
    for key in ["id", "name", "schema_version"] {
        if let Some(v) = value.get(key) {
            flat.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(flat)
}

fn process_profile_from_value(value: &Value) -> Option<JobProfile> {
    // v3 machining profiles nest the setup under steps[]. Flatten step 0 up beside
    // id/name so this (currently single-setup) projection reads it as before.
    // Multi-step projection lands with the step editor + planner.
    let flattened = flatten_first_step(value);
    let value = &flattened;

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

fn load_persisted_unit_system() -> UserUnitSystem {
    let Some(state) = persistence_state() else {
        return UserUnitSystem::Metric;
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

    UserUnitSystem::from_settings_str(units_value)
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

#[cfg(test)]
mod step_projection_tests {
    use super::*;

    /// A v3 stepped machining value, mirroring what AppData yields.
    fn stepped_machining(cnc: &str) -> Value {
        json!({
            "schema_version": 3,
            "id": "018f0000-0000-7000-8000-000000000001",
            "name": "PTH board",
            "steps": [
                {
                    "name": "Drill PTH",
                    "cnc": { "default": cnc, "choices": [cnc] },
                    "fixture": { "default": cnc, "choices": [cnc] },
                    "toolset": { "default": cnc, "choices": [cnc] },
                    "side_to_machine": "top",
                    "operations": ["drill_pth", "route_board"],
                }
            ]
        })
    }

    #[test]
    fn flatten_first_step_lifts_step_zero_beside_identity() {
        let cnc = "018f0000-0000-7000-8000-0000000000aa";
        let flat = flatten_first_step(&stepped_machining(cnc));
        assert_eq!(flat.get("name").and_then(Value::as_str), Some("PTH board"));
        assert_eq!(flat.pointer("/cnc/default").and_then(Value::as_str), Some(cnc));
        assert!(flat.get("operations").and_then(Value::as_array).is_some());
    }

    #[test]
    fn process_profile_from_value_reads_the_first_step() {
        // The single-setup projection sources cnc/operations from step 0.
        let cnc = "018f0000-0000-7000-8000-0000000000bb";
        let profile = process_profile_from_value(&stepped_machining(cnc)).expect("projects");
        assert_eq!(profile.cnc_profile_id, cnc);
        assert!(profile.default_operations.contains(&ProductionOperation::DrillPth));
        assert!(profile.default_operations.contains(&ProductionOperation::RouteBoard));
    }
}
