struct RuntimeIssueDraft {
    domain: String,
    owner_tag: Option<String>,
    message: String,
    details: Option<String>,
}

/// The diagnostics domain generation failures live in.
///
/// One domain for the whole run, so each publish replaces the previous run's entries
/// wholesale — which is what makes the banner clear itself the moment a run succeeds.
pub const GENERATION_ERROR_DOMAIN: &str = "generation";

/// The diagnostics domain the readiness gate's no-go reasons live in.
///
/// Held apart from [`GENERATION_ERROR_DOMAIN`] because the two say different things and
/// are replaced on different events: that one is *the last run failed*, published when a
/// run finishes; this one is *a run cannot start*, published every time the gate is
/// evaluated. Sharing a domain would have each clearing the other's entries.
pub const READINESS_ERROR_DOMAIN: &str = "readiness";

impl AppState {
    // Creates runtime defaults, then hydrates persisted data from disk.
    pub fn new(boot: &UiLaunchData) -> Self {
        let tools = vec![];

        let mut state = Self {
            selected_screen: Screen::Job,
            selected_job_view: JobCenterView::Board,
            selected_step: 0,
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
            // No programs until generation actually runs (kicked at launch when the
            // job is ready; re-run on every mutation). Seeding a canned sample here
            // made the Code view show fake GCode that was never generated.
            programs: Vec::new(),
            suppress_persistence: false,
            show_first_launch: true,
            rack_slots: BTreeMap::new(),
            board: boot.board_snapshot.clone(),
            kicad_status: boot.kicad_status.clone(),
            gcode_save_directory: load_persisted_string("gcode_save_directory"),
            last_removable_media_path: load_persisted_string("last_removable_media_path"),
            job_view_pinned: load_persisted_flag("job_view_pinned", false),
            job_pin_width: load_persisted_job_pin_width(),
            window_width: load_persisted_window_dimension(
                "window_width",
                DEFAULT_WINDOW_WIDTH,
                MIN_WINDOW_WIDTH,
            ),
            window_height: load_persisted_window_dimension(
                "window_height",
                DEFAULT_WINDOW_HEIGHT,
                MIN_WINDOW_HEIGHT,
            ),
            window_maximized: load_persisted_flag("window_maximized", false),
            // Both default to `true` when the key is absent, so a settings file
            // written before these existed opts *in* rather than silently out.
            update_check_enabled: load_persisted_flag("update_check_enabled", true),
            update_last_check: load_persisted_string("update_last_check"),
            update_skipped_version: load_persisted_string("update_skipped_version"),
            update_postponed_until: load_persisted_string("update_postponed_until"),
            security_log_enabled: load_persisted_flag("security_log_enabled", true),
            available_update: None,
            update_installing: false,
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
            "gcode_save_directory": self.gcode_save_directory,
            "last_removable_media_path": self.last_removable_media_path,
            "job_view_pinned": self.job_view_pinned,
            "job_pin_width": self.job_pin_width,
            "window_width": self.window_width,
            "window_height": self.window_height,
            "window_maximized": self.window_maximized,
            "update_check_enabled": self.update_check_enabled,
            "update_last_check": self.update_last_check,
            "update_skipped_version": self.update_skipped_version,
            "update_postponed_until": self.update_postponed_until,
            "security_log_enabled": self.security_log_enabled,
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
        self.push_runtime_error_quiet(domain, owner_tag, message.clone(), details);
        self.log_event(message);
    }

    /// Records a diagnostic **without** also raising a toast for it.
    ///
    /// Separated from [`Self::push_runtime_error_owned`] for callers that already say their
    /// piece once: a generation run reports a single summary, and one toast per failed step
    /// on top of it would bury that summary under the detail it is summarising.
    fn push_runtime_error_quiet(
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
            // Suffixed with the position so several entries pushed in the same millisecond
            // — every failed step of one run — keep distinct ids for the UI's keying.
            id: format!("err-{}-{}", created_ms, self.errors.len()),
            domain: domain.to_string(),
            owner_tag,
            is_error: true,
            message,
            details,
        });
        const MAX_ERRORS: usize = 200;
        if self.errors.len() > MAX_ERRORS {
            let drop_count = self.errors.len() - MAX_ERRORS;
            self.errors.drain(0..drop_count);
        }
    }

    fn clear_runtime_errors(&mut self, domain: &str) {
        self.errors.retain(|error| error.domain != domain);
    }

    /// Replaces the standing generation diagnostics with `failures` (`(headline, detail)`).
    ///
    /// Generation failure used to be reported *only* by `log_event` — a toast that fades
    /// after a few seconds. But a failed run is a **standing condition**: it stays wrong
    /// until the operator changes something, and `programs` has been cleared, so nothing
    /// else on screen says why there is no G-code. An operator who looked away for ten
    /// seconds was left with an empty Code view and no explanation.
    ///
    /// Being a domain replace, this also *clears* itself: pass an empty list on a good run
    /// and the banner goes away without anyone having to remember to dismiss it.
    pub fn set_generation_errors(&mut self, failures: Vec<(String, Option<String>)>) {
        self.clear_runtime_errors(GENERATION_ERROR_DOMAIN);
        for (message, details) in failures {
            self.push_runtime_error_quiet(GENERATION_ERROR_DOMAIN, None, message, details);
        }
    }

    /// Replaces the standing readiness diagnostics with the gate's current no-go reasons.
    ///
    /// The same argument as [`Self::set_generation_errors`], applied to the other half of
    /// the problem. A *failed* run was a standing condition and became a banner entry; a
    /// run that never started was left as a `log::warn!` and a pill reading "Not ready",
    /// so the only way to learn why the application was refusing to work was to open the
    /// Logs screen and read it. The reasons were already rendered in one place — the Job's
    /// Code tab — but only there, and only once the operator had thought to look.
    ///
    /// Anything that holds the gate shut belongs on screen, wherever the operator is
    /// standing. That is the whole rule, and this is what applies it.
    ///
    /// Like its sibling this is a domain replace, so it clears itself: a gate that opens
    /// publishes an empty list and the banner goes away with nobody having to dismiss it.
    ///
    /// Quiet (no toast) deliberately. The gate is re-evaluated on every mutation, so the
    /// same unchanged reason would raise a toast on each keystroke of an unrelated edit.
    ///
    /// Idempotent for the same reason: an unchanged set is left alone rather than cleared
    /// and re-pushed. Re-pushing would mint fresh ids on every mutation, and those ids key
    /// the banner's detail list — so an operator with the details open would have the
    /// entry they were reading torn down and rebuilt under them as they typed elsewhere.
    pub fn set_readiness_errors(&mut self, reasons: &[String]) {
        let unchanged = self
            .errors
            .iter()
            .filter(|error| error.domain == READINESS_ERROR_DOMAIN)
            .map(|error| error.message.as_str())
            .eq(reasons.iter().map(String::as_str));
        if unchanged {
            return;
        }

        self.clear_runtime_errors(READINESS_ERROR_DOMAIN);
        for reason in reasons {
            self.push_runtime_error_quiet(
                READINESS_ERROR_DOMAIN,
                None,
                reason.clone(),
                Some("The job cannot be generated until this is resolved.".to_string()),
            );
        }
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

    /// Where a G-code Save dialog should open.
    ///
    /// The remembered directory wins, but only while it still exists — a folder that
    /// has since been deleted, renamed, or lived on a drive that is no longer mounted
    /// would otherwise open the dialog somewhere useless. Anything else (including the
    /// very first save) falls back to the host's download folder, which is where a
    /// desktop user expects a generated file to land.
    pub fn gcode_save_directory_or_default(&self) -> std::path::PathBuf {
        resolve_save_directory(self.gcode_save_directory.as_deref())
    }

    /// Toggles the docked Job view and persists the choice.
    pub fn toggle_job_view_pinned(&mut self) {
        self.job_view_pinned = !self.job_view_pinned;
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Records the chosen palette.
    ///
    /// Light and Dark only, with no "follow the system": a machining session runs for
    /// hours, and a window that repaints itself when the desktop crosses into evening
    /// is a surprise mid-job rather than a convenience.
    ///
    /// The equality guard is what lets the settings dialog wire this straight to a
    /// segmented control — re-picking the palette already in use writes nothing.
    pub fn set_theme(&mut self, theme: Theme) {
        if self.theme == theme {
            return;
        }
        self.theme = theme;
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Records the docked column's width after a split-handle drag. Called on release
    /// rather than on every mouse move, so a drag is one settings write, not hundreds.
    pub fn set_job_pin_width(&mut self, width: i64) {
        let width = width.max(MIN_JOB_PIN_WIDTH);
        if self.job_pin_width == width {
            return;
        }
        self.job_pin_width = width;
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Records the window's inner size and maximized state, so the next launch reopens
    /// the window the user last left (see [`crate::ui::window_state`]).
    ///
    /// `size` is `None` while the window is maximized: the inner size then *is* the
    /// screen, and storing it would make the next un-maximize yield a screen-sized
    /// window that is no longer maximized. The stored restore size is kept instead.
    ///
    /// A no-op when nothing changed, so the resize stream a drag produces collapses to
    /// one settings write per distinct size rather than one per frame.
    pub fn set_window_geometry(&mut self, size: Option<(i64, i64)>, maximized: bool) {
        let mut changed = false;

        if let Some((width, height)) = size {
            let width = width.clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_DIMENSION);
            let height = height.clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_DIMENSION);
            if (self.window_width, self.window_height) != (width, height) {
                self.window_width = width;
                self.window_height = height;
                changed = true;
            }
        }

        if self.window_maximized != maximized {
            self.window_maximized = maximized;
            changed = true;
        }

        if changed {
            self.persist_realms(&[PersistRealm::GlobalSettings]);
        }
    }

    /// Records the directory a save wrote to and mirrors it to `global.setting.yaml`.
    /// A no-op when the directory is unchanged, so re-saving the same file does not
    /// churn the settings write.
    pub fn remember_gcode_save_directory(&mut self, directory: &std::path::Path) {
        let directory = directory.to_string_lossy().into_owned();
        if self.gcode_save_directory.as_deref() == Some(directory.as_str()) {
            return;
        }
        self.gcode_save_directory = Some(directory);
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Records the removable-media directory a "Save to USB" wrote to.
    ///
    /// Deliberately *not* folded into [`Self::remember_gcode_save_directory`]: the two
    /// have independent histories, and letting a USB save move the ordinary dialog to a
    /// drive letter that will be gone tomorrow is exactly the annoyance this feature
    /// exists to remove. Same no-op-when-unchanged contract, for the same reason.
    pub fn remember_removable_media_path(&mut self, directory: &std::path::Path) {
        let directory = directory.to_string_lossy().into_owned();
        if self.last_removable_media_path.as_deref() == Some(directory.as_str()) {
            return;
        }
        self.last_removable_media_path = Some(directory);
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    // -----------------------------------------------------------------------
    // Update-check preferences
    //
    // The four setters below are the whole of EU CRA Annex I (2)(c)'s user-facing
    // contract: checks are on by default, the user is *notified* rather than
    // surprised, they may "temporarily postpone", and they have a clear opt-out.
    // Postpone and skip are deliberately separate — see the doc comments.
    // -----------------------------------------------------------------------

    /// Turns the daily update check on or off.
    ///
    /// Switching it off also clears the postpone and skip stamps: they exist only to
    /// suppress a banner that can no longer appear, and leaving them set would make a
    /// later re-enable behave according to a decision the user made long ago about a
    /// version that has since shipped.
    pub fn set_update_check_enabled(&mut self, enabled: bool) {
        if self.update_check_enabled == enabled {
            return;
        }
        self.update_check_enabled = enabled;
        if !enabled {
            self.update_postponed_until = None;
            self.update_skipped_version = None;
        }
        self.persist_realms(&[PersistRealm::GlobalSettings]);
        security_log::record_ok(
            security_log::Event::UpdateCheckSettingChanged,
            serde_json::json!({ "enabled": enabled }),
        );
    }

    /// Stamps a completed check, whatever its outcome.
    ///
    /// Called on failure too (offline, rate-limited, malformed response). Stamping only
    /// on success would make a machine with no network retry on every single launch,
    /// which is both useless and the closest thing to abusive traffic this app can emit.
    pub fn record_update_check_now(&mut self) {
        self.update_last_check = Some(chrono::Utc::now().to_rfc3339());
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Silences *every* update banner for `days` — the "remind me later" action.
    pub fn postpone_update(&mut self, days: i64) {
        let until = chrono::Utc::now() + chrono::Duration::days(days);
        self.update_postponed_until = Some(until.to_rfc3339());
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Silences one specific version forever — the "skip this version" action.
    ///
    /// Distinct from [`Self::postpone_update`] on purpose: postponing is about *when*
    /// the user wants to be asked, skipping is about *which release* they have decided
    /// against. A skipped `0.9.1` must not stop `0.9.2` from being announced, and only
    /// storing the version (rather than a flag) gets that for free.
    pub fn skip_update_version(&mut self, version: impl Into<String>) {
        self.update_skipped_version = Some(version.into());
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Undoes both suppressions, so the next check announces whatever it finds.
    ///
    /// One action rather than two: the Settings screen shows whichever suppressions are
    /// active, and a user clearing either one is saying "start telling me about updates
    /// again". Clearing only the half they clicked would leave the other silently in
    /// force and the banner still absent, which reads as the button not working.
    pub fn clear_update_suppressions(&mut self) {
        if self.update_postponed_until.is_none() && self.update_skipped_version.is_none() {
            return;
        }
        self.update_postponed_until = None;
        self.update_skipped_version = None;
        self.persist_realms(&[PersistRealm::GlobalSettings]);
    }

    /// Turns the persisted security log on or off (EU CRA Annex I (2)(l) opt-out).
    ///
    /// Returns the previous value so the caller can record the transition *before* the
    /// writer stops: an opt-out that leaves no trace of itself makes the log's own
    /// gaps unexplainable, which is exactly what an audit record must not do.
    pub fn set_security_log_enabled(&mut self, enabled: bool) -> bool {
        let previous = self.security_log_enabled;
        if previous == enabled {
            return previous;
        }

        // Record the transition while the writer is still open, then close it. An
        // opt-out that leaves no trace of itself makes the record's own gap
        // unexplainable — a reader cannot tell "nothing happened" from "recording
        // stopped", and that ambiguity is exactly what an audit trail must not have.
        if !enabled {
            security_log::record_ok(
                security_log::Event::SecurityLogSettingChanged,
                serde_json::json!({ "enabled": false }),
            );
        }

        self.security_log_enabled = enabled;
        security_log::set_enabled(enabled);

        if enabled {
            security_log::record_ok(
                security_log::Event::SecurityLogSettingChanged,
                serde_json::json!({ "enabled": true }),
            );
        }

        self.persist_realms(&[PersistRealm::GlobalSettings]);
        previous
    }

    /// The file name a G-code Save should offer: the board's name (KiCad's file stem,
    /// so `panel.kicad_pcb` becomes `panel`) plus the program extension. Falls back to
    /// a generic stem when no board is loaded, so the dialog is never blank.
    pub fn gcode_default_file_name(&self) -> String {
        self.program_file_name(0, 1, "", GCODE_FILE_EXTENSION)
    }

    /// What the program for `index` is saved as.
    ///
    /// A one-step job is named for the board alone — a `_step1` suffix on the only file
    /// there is would be noise, and the whole point of the single-step case is that
    /// nothing hints steps exist.
    ///
    /// Beyond one, the step is named by **the operator's own name for it** — a stick
    /// holding `panel_Drill PTH.nc` and `panel_Route outline.nc` says what to load when,
    /// which `panel_step1.nc` and `panel_step2.nc` do not, and the operator is the one
    /// standing at the machine deciding. The ordinal is the fallback for a step left
    /// unnamed, and for one whose name survives sanitising as nothing at all (a step
    /// called `?` or `...`).
    ///
    /// Sanitised the same way the board name is, so one convention governs the whole
    /// file name: characters Windows forbids become `_`, and spaces and case are kept
    /// (`panel v2` already survives here, so a step name should not be held to a
    /// stricter rule than the board is).
    ///
    /// **Uniqueness is the caller's** — step names need not be unique, and this cannot
    /// see its siblings. See `save_rows`, which drops the colliding ones back to their
    /// ordinals.
    pub fn program_file_name(
        &self,
        index: usize,
        step_count: usize,
        step_name: &str,
        extension: &str,
    ) -> String {
        let stem = self
            .board
            .as_ref()
            .map(|board| board.name.trim())
            .filter(|name| !name.is_empty())
            .map(sanitize_file_stem)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "program".to_string());
        let extension = extension.trim().trim_start_matches('.');
        let extension = if extension.is_empty() { GCODE_FILE_EXTENSION } else { extension };
        if step_count <= 1 {
            return format!("{stem}.{extension}");
        }
        // Falls back on "carries no letter or digit" rather than on "is empty", because
        // sanitising replaces a forbidden character rather than dropping it: a step called
        // `?` comes back as `_`, which is not empty and would name the file `panel__.nc`.
        // A suffix made entirely of punctuation tells the operator nothing the ordinal
        // does not tell them better.
        let named = sanitize_file_stem(step_name);
        let suffix = if named.chars().any(|c| c.is_alphanumeric()) {
            named
        } else {
            format!("step{}", index + 1)
        };
        format!("{stem}_{suffix}.{extension}")
    }

    /// The program for the step the Job views are showing, if that step has one.
    pub fn selected_program(&self) -> Option<&StepProgram> {
        self.programs.get(self.selected_step)
    }

    /// Every step that produced a program, paired with it — what the save flow offers.
    /// A failed step is simply absent here; its reason is on the Code view.
    pub fn ready_programs(&self) -> Vec<(&StepProgram, &Program)> {
        self.programs.iter().filter_map(|step| step.program().map(|p| (step, p))).collect()
    }

    /// Whether there is anything at all to save.
    pub fn has_any_program(&self) -> bool {
        self.programs.iter().any(|step| step.program().is_some())
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
    pub fn validate_tooling(&mut self, stitched: Option<&pcb::StitchResult>) {
        let failures: Vec<(String, Vec<String>)> = if crate::data::appdata_ready() {
            crate::runtime::tooling::plan_tooling(self, stitched)
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

    /// Rebuilds the in-memory fixture list from the `AppData`-owned fixture
    /// documents. AppData persists that realm; this projection keeps the legacy
    /// consumers — the current-job reference check and the setup screen — coherent
    /// while the two layers coexist. Without it, a fixture created mid-session
    /// (the launch-time `PERSISTENCE_STATE` snapshot is frozen) never reaches
    /// `self.fixtures`, so a machining profile that references it is wrongly
    /// reported as a broken fixture reference. A selection whose fixture no longer
    /// exists is repointed. Does not itself persist (AppData already wrote the file).
    pub fn refresh_fixtures(&mut self, values: &[Value]) {
        let fixtures: Vec<FixtureProfile> =
            values.iter().filter_map(fixture_profile_from_value).collect();

        let ids: BTreeSet<String> = fixtures.iter().map(|f| f.id.clone()).collect();
        if let Some(selected) = self.selected_fixture_id.clone() {
            if !ids.contains(&selected) {
                self.selected_fixture_id = fixtures.first().map(|f| f.id.clone());
            }
        }

        self.fixtures = fixtures;
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
                        // Not carried by `CatalogStockTool`, like `flute_length` beside it: the
            // catalogue knows them, the add-from-catalogue projection does not.
            flute_length: None,
            tip_diameter: None,
            z_min_depth: None,
                        table_feed: tool.table_feed,
                        catalog_table_feed: tool.table_feed,
                        z_feed: tool.z_feed,
                        catalog_z_feed: tool.z_feed,
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
            "spindle_rpm_min": machine.spindle_rpm_min.to_string(),
            "spindle_rpm_max": machine.spindle_rpm_max.to_string(),
            "max_feed_xy": machine.max_feed_xy.to_string(),
            "max_feed_z": machine.max_feed_z.to_string(),
            "output_file_extension": machine.output_file_extension,
            "atc_slot_count": machine.atc_slot_count,
            "curve_tolerance": machine.curve_tolerance.to_string(),
            "scaling": {
                "x": machine.scaling_x,
                "y": machine.scaling_y,
            },
            // In the fingerprint because it decides whether `tool_measure` is emitted, so
            // flipping it must retrigger generation.
            "tool_length_measurement": if machine.measures_tool_length { "manual" } else { "auto_setter" },
        },
        // Keys in the schema's own order, and each field named after the key it carries —
        // this crosswalk used to map `linear_cut` onto `drill_cycle_mode_series` and
        // `banner` onto `route_retract`, names from a design that no longer exists.
        "primitives": {
            "program_begin": machine.program_begin_tpl,
            "program_end": machine.program_end_tpl,
            "set_unit": machine.set_unit_tpl,
            "set_origin": machine.set_origin_tpl,
            "tool_change": machine.tool_change_tpl,
            "tool_measure": machine.tool_measure_tpl,
            "spindle_start": machine.spindle_start_tpl,
            "spindle_stop": machine.spindle_stop_tpl,
            "move_rapid": machine.move_rapid_tpl,
            "cut_linear": machine.cut_linear_tpl,
            "cut_arc": machine.cut_arc_tpl,
            "drill": machine.drill_tpl,
            "comment": machine.comment_tpl,
            "message": machine.message_tpl,
            "pause": machine.pause_tpl,
            "line_format": machine.line_format_tpl,
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

/// What a CNC profile must carry for the readiness gate to let a job generate.
///
/// The machine fields are stated here rather than taken from the schema on purpose: they
/// are a deliberate **superset** of `cnc.yaml`'s own `machine.required` (which asks only
/// for the spindle range). The rest carry schema defaults the loader materialises, and the
/// generator reads every one of them, so a profile that somehow lacks one cannot produce a
/// program even though the schema would accept it.
///
/// The **primitives** are not restated — they come from
/// [`required_primitives`](crate::gcode::primitive_vars::required_primitives), which reads
/// the schema. A restatement lived here and drifted: it still named the pre-rename
/// primitives long after every profile had been migrated off them, which shut the gate on
/// every job in existence. There is nothing to keep in step now.
fn machine_required_paths() -> &'static [String] {
    static PATHS: OnceLock<Vec<String>> = OnceLock::new();
    PATHS.get_or_init(|| {
        let mut paths: Vec<String> = [
            "id",
            "machine.spindle_rpm_min",
            "machine.spindle_rpm_max",
            "machine.max_feed_xy",
            "machine.max_feed_z",
            "machine.output_file_extension",
            "machine.atc_slot_count",
            "machine.scaling.x",
            "machine.scaling.y",
        ]
        .iter()
        .map(|path| (*path).to_string())
        .collect();
        paths.extend(
            crate::gcode::primitive_vars::required_primitives()
                .iter()
                .map(|name| format!("primitives.{name}")),
        );
        paths
    })
}

fn fixture_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "name",
        "board_holding_method",
        "origin",
    ]
}

fn process_required_paths() -> &'static [&'static str] {
    &["id", "name", "cnc", "fixture", "toolset", "operations"]
}

fn toolset_required_paths() -> &'static [&'static str] {
    &[
        "id",
        "name",
        "generation_policy",
        "slots",
    ]
}

/// Generic over the path type so a hand-written `&[&str]` and a schema-derived
/// `&[String]` can both be passed — see [`machine_required_paths`].
fn collect_missing_required<S: AsRef<str>>(value: &Value, required_paths: &[S]) -> BTreeSet<String> {
    required_paths
        .iter()
        .map(AsRef::as_ref)
        .filter(|path| !has_path(value, path))
        .map(str::to_string)
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

    // The schema defaults both to 5000mm/min and the parser fills that in for a profile
    // written before they existed, so the fallback here is only reached by a value the
    // unit parser rejects. It matches the schema rather than inventing a third number.
    let feed_at = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .and_then(|raw| units::FeedRate::from_string(raw, Some(units::FeedRateUnit::MmPerMin)).ok())
            .unwrap_or_else(|| units::FeedRate::from_mm_per_min(5000.0))
    };
    let max_feed_xy = feed_at("/machine/max_feed_xy");
    let max_feed_z = feed_at("/machine/max_feed_z");

    // Defaulted by the schema and backfilled on load, so the fallback here is only
    // reached by a value the pattern rejects. Matches the schema rather than inventing
    // a third answer.
    let output_file_extension = value
        .pointer("/machine/output_file_extension")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .unwrap_or(GCODE_FILE_EXTENSION)
        .to_string();

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

    // One template, by the name the schema gives it. Absent reads as empty, which is a
    // legitimate state for every optional primitive (and, for a required one, a profile
    // the schema would already have rejected).
    let primitive = |name: &str| -> String {
        value
            .pointer(&format!("/primitives/{name}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    Some(MachineProfile {
        id,
        name,
        spindle_rpm_min,
        spindle_rpm_max,
        max_feed_xy,
        max_feed_z,
        output_file_extension,
        atc_slot_count,
        // Schema-defaulted and materialised by the loader, so the fallback here is only
        // reached by a value the unit parser rejects. Matches the schema rather than
        // inventing a third answer.
        curve_tolerance: value
            .pointer("/machine/curve_tolerance")
            .and_then(Value::as_str)
            .and_then(|raw| Length::from_string(raw, Some(units::LengthUnit::Mm)).ok())
            .unwrap_or_else(|| Length::from_mm(0.01)),
        scaling_x,
        scaling_y,
        line_format_tpl: primitive("line_format"),
        set_unit_tpl: primitive("set_unit"),
        set_origin_tpl: primitive("set_origin"),
        // Each primitive reads from `/primitives/<its own name>` and nowhere else.
        //
        // Every one of these carried a chain of `or_else` fallbacks onto older document
        // shapes (`/templates/…`, `/drill/cycle_start`, a bare top-level key). None could
        // fire: this crosswalk is only ever handed a datastore-parsed document, and the
        // schema's `additionalProperties: false` rejects a file carrying any of those
        // shapes long before it reaches here. Dead chains that *look* like compatibility
        // are worse than none — they suggest a file format is still supported when
        // opening one would in fact be refused. Migration is `normalize_cnc_value`'s job.
        program_begin_tpl: primitive("program_begin"),
        program_end_tpl: primitive("program_end"),
        tool_change_tpl: primitive("tool_change"),
        tool_measure_tpl: primitive("tool_measure"),
        spindle_start_tpl: primitive("spindle_start"),
        spindle_stop_tpl: primitive("spindle_stop"),
        move_rapid_tpl: primitive("move_rapid"),
        cut_linear_tpl: primitive("cut_linear"),
        cut_arc_tpl: primitive("cut_arc"),
        drill_tpl: primitive("drill"),
        comment_tpl: primitive("comment"),
        message_tpl: primitive("message"),
        pause_tpl: primitive("pause"),
        // `auto_setter` measures at M06 and needs no block; `manual` is the case that
        // does. Defaulting to the schema's own `manual` would make every profile emit a
        // measurement it may not want, so an unreadable value reads as "no".
        measures_tool_length: value
            .pointer("/machine/tool_length_measurement")
            .and_then(Value::as_str)
            .is_some_and(|mode| mode == "manual"),
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

fn fixture_profile_to_value(fixture: &FixtureProfile) -> Value {
    // Change-detection fingerprint (not the persistence writer — AppData owns the
    // file). It must include every field generation depends on, so editing a Z value
    // re-triggers a run; the Z fields feed the depth math and bed-safety check.
    json!({
        "schema_version": 1,
        "id": fixture.id,
        "name": fixture.name,
        "board_holding_method": fixture.backing_board,
        "origin": {
            "x0": fixture.origin_x0,
            "y0": fixture.origin_y0,
        },
        "backboard_thickness": fixture.backboard_thickness.to_string(),
        "bed_clearance": fixture.bed_clearance.to_string(),
        "breakthrough": fixture.breakthrough.to_string(),
        "z_retract": fixture.z_retract.to_string(),
        "z_safe": fixture.z_safe.to_string(),
        "origin_reference": fixture.origin_reference,
        // In the fingerprint because generation depends on it: it decides which axis a
        // bottom-side step mirrors about, so changing it moves every hole on the solder
        // side. Absent from here, the operator would fix a mirrored board by correcting
        // the axis and watch nothing regenerate.
        "board_flip_axis": fixture.board_flip_axis,
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
        backing_board: value
            .get("board_holding_method")
            .and_then(Value::as_str)
            .or_else(|| value.get("backing_board").and_then(Value::as_str))
            .unwrap_or("MDF spoilboard")
            .to_string(),
        // The Z model (schemas/fixture.yaml): all measured from the board top (Z0).
        // Each carries a schema default the datastore materialises on load, so the
        // mm fallbacks here are only a guard against a hand-edited file.
        backboard_thickness: size_at(value, "/backboard_thickness", 2.5),
        bed_clearance: size_at(value, "/bed_clearance", 0.5),
        breakthrough: size_at(value, "/breakthrough", 0.5),
        z_retract: size_at(value, "/z_retract", 5.0),
        z_safe: size_at(value, "/z_safe", 20.0),
        // The registered corner. Schema-defaulted, so the fallbacks here only guard a
        // hand-edited file; `normalize_fixture_value` has already corrected any value
        // that names an edge the axis cannot be zeroed on.
        origin_x0: value
            .pointer("/origin/x0")
            .and_then(Value::as_str)
            .unwrap_or("left")
            .to_string(),
        origin_y0: value
            .pointer("/origin/y0")
            .and_then(Value::as_str)
            .unwrap_or("front")
            .to_string(),
        // `y` — the page turn — is what the schema assumes for a profile written before
        // this field existed, so it is also the fallback here.
        board_flip_axis: value
            .get("board_flip_axis")
            .and_then(Value::as_str)
            .unwrap_or("y")
            .to_string(),
        // Carried through exactly as entered — trimming or upper-casing it here would
        // take the decision away from the CNC profile's `set_origin`, which needs the
        // original text to quote back when it rejects one. Empty is a legitimate state
        // (unset), and `set_origin` is what refuses to generate against it.
        origin_reference: value
            .get("origin_reference")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pending_required_fields: pending_required_fields.clone(),
        usable: pending_required_fields.is_empty(),
    })
}

/// Reads a size (length) at `pointer`, falling back to `default_mm` when the field is
/// absent or unparseable. Used for the fixture Z fields, which all carry schema
/// defaults (so the fallback rarely fires).
fn size_at(value: &Value, pointer: &str, default_mm: f64) -> Length {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(|raw| units::Length::from_string(raw, Some(units::LengthUnit::Mm)).ok())
        .unwrap_or_else(|| Length::from_mm(default_mm))
}

fn process_profile_to_value(profile: &JobProfile) -> Value {
    let mut value = json!({
        "schema_version": 2,
        "id": profile.id,
        "name": profile.name,
        "board_face": profile.board_face.as_str(),
        "cnc": profile.cnc_profile_id,
        "fixture": profile.fixture_profile_id,
        "toolset": profile.toolset_profile_id,
        "operations": profile
            .default_operations
            .iter()
            .map(|op| operation_to_key(*op))
            .collect::<Vec<_>>(),
        "routing": {
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

    // `side_to_machine: top | bottom` was renamed to `board_face: front | back`; the
    // loader migrates stored documents, but this crosswalk also sees values built in
    // memory, so it reads both spellings rather than silently defaulting an old one to
    // the front.
    let board_face = match value
        .get("board_face")
        .or_else(|| value.get("side_to_machine"))
        .and_then(Value::as_str)
        .unwrap_or("front")
    {
        face if face.eq_ignore_ascii_case("back") || face.eq_ignore_ascii_case("bottom") => {
            BoardFace::Back
        }
        _ => BoardFace::Front,
    };

    let cnc_profile_id = value
        .get("cnc")
        .and_then(Value::as_str)
        .or_else(|| value.get("cnc_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    let fixture_profile_id = value
        .get("fixture")
        .and_then(Value::as_str)
        .or_else(|| value.get("fixture_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

    let toolset_profile_id = value
        .get("toolset")
        .and_then(Value::as_str)
        .or_else(|| value.get("toolset_profile_id").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();

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

    // Each binding is one optional reference. Absent means the operator has not chosen
    // one yet — a pending field, so the step reads as incomplete rather than being
    // silently defaulted onto some other machine. A present-but-malformed reference is
    // a corrupt file, and skipping the profile is the honest response to that.
    for (field, value) in [
        ("cnc", &cnc_profile_id),
        ("fixture", &fixture_profile_id),
        ("toolset", &toolset_profile_id),
    ] {
        if value.trim().is_empty() {
            pending_required_fields.insert(field.to_string());
        } else if !is_uuid(value) {
            warn!("Skipping machining profile '{id}': {field} is not a UUID ({value})");
            return None;
        }
    }
    if default_operations.is_empty() {
        pending_required_fields.insert("operations".to_string());
    }

    Some(JobProfile {
        id,
        name,
        cnc_profile_id,
        fixture_profile_id,
        toolset_profile_id,
        board_face,
        default_operations,
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
        ProductionOperation::RouteCutouts => json!({
            "enabled": false,
            "retain_island": true,
            "island_tab": "4%",
            "drill_sharp_corners": true,
        }),
        ProductionOperation::MillBoard => json!({
            "enabled": false,
            "finishing": {
                "clearance": "0.1mm",
                "direction": "climb",
            }
        }),
        ProductionOperation::EngraveCopper => json!({
            "enabled": false,
            "width": "0.25mm",
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
        ProductionOperation::RouteCutouts => "route_cutouts",
        ProductionOperation::MillBoard => "mill_board",
        ProductionOperation::EngraveCopper => "engrave_copper",
    }
}

fn operation_from_key(value: &str) -> Option<ProductionOperation> {
    match value {
        "drill_locating_pins" => Some(ProductionOperation::DrillLocatingPins),
        "drill_pth" => Some(ProductionOperation::DrillPth),
        "drill_npth" => Some(ProductionOperation::DrillNpth),
        "route_board" => Some(ProductionOperation::RouteBoard),
        "route_cutouts" => Some(ProductionOperation::RouteCutouts),
        "mill_board" => Some(ProductionOperation::MillBoard),
        "engrave_copper" => Some(ProductionOperation::EngraveCopper),
        _ => None,
    }
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

/// Picks the directory a Save dialog opens in: the remembered one while it still
/// exists, else the host default. Split out from [`AppState`] so the "destination has
/// gone away" path is testable without building a whole app state.
fn resolve_save_directory(remembered: Option<&str>) -> std::path::PathBuf {
    remembered
        .filter(|dir| !dir.trim().is_empty())
        .map(std::path::PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(host_default_save_directory)
}

/// The host's default place for a user-generated file. `dirs` resolves this properly
/// per platform — Windows reads the `Downloads` known folder from the shell rather
/// than assuming `%USERPROFILE%\Downloads` (it is relocatable), macOS gives
/// `~/Downloads`, and Linux honours `XDG_DOWNLOAD_DIR`. Falls back to the home
/// directory, then the working directory, so this always yields *somewhere*.
fn host_default_save_directory() -> std::path::PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Strips characters Windows forbids in a file name (and that the other platforms are
/// merely unhappy about), so a board named `panel v2: rev/3` still yields a usable
/// default. Only the *stem* is sanitised; the extension is added by the caller.
fn sanitize_file_stem(stem: &str) -> String {
    stem.chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_end_matches('.') // Windows silently drops a trailing dot
        .to_string()
}

/// A persisted boolean settings flag, falling back to `default` when the key is
/// absent or holds a non-boolean.
///
/// `default` is a parameter rather than a fixed `false` because the opt-out flags
/// must read as *enabled* when missing: a settings file written before
/// `update_check_enabled` existed has to mean "checks on", not "user opted out".
fn load_persisted_flag(key: &str, default: bool) -> bool {
    persistence_state()
        .and_then(|state| state.global_settings.get(key).and_then(Value::as_bool))
        .unwrap_or(default)
}

/// The persisted docked-column width, floored so a hand-edited settings file cannot
/// produce an unreadably narrow column. Not ceilinged: the layout reserves the screen's
/// width and gives the dock the rest, so an over-large stored value simply means "as
/// wide as it goes" (see [`crate::runtime::MIN_JOB_PIN_WIDTH`]).
fn load_persisted_job_pin_width() -> i64 {
    persistence_state()
        .and_then(|state| state.global_settings.get("job_pin_width").and_then(Value::as_i64))
        .unwrap_or(DEFAULT_JOB_PIN_WIDTH)
        .max(MIN_JOB_PIN_WIDTH)
}

/// A persisted window dimension, clamped so a stale or hand-edited settings file cannot
/// open a window too small to operate (or absurdly large). `minimum` differs per axis;
/// the upper rail does not.
fn load_persisted_window_dimension(key: &str, default: i64, minimum: i64) -> i64 {
    persistence_state()
        .and_then(|state| state.global_settings.get(key).and_then(Value::as_i64))
        .unwrap_or(default)
        .clamp(minimum, MAX_WINDOW_DIMENSION)
}

/// A non-empty string from the settings file, if it carries one under `key`.
///
/// Blank is treated as absent everywhere this is used. For a remembered directory that
/// matters concretely — a file hand-edited to `""` would otherwise open a dialog at the
/// process's current directory, which is wherever the app happened to be launched from.
/// For the update timestamps it means a blank stamp re-checks rather than parsing to a
/// bogus instant.
fn load_persisted_string(key: &str) -> Option<String> {
    persistence_state()?
        .global_settings
        .get(key)
        .and_then(Value::as_str)
        .filter(|dir| !dir.trim().is_empty())
        .map(str::to_string)
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
                    "cnc": cnc,
                    "fixture": cnc,
                    "toolset": cnc,
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
        assert_eq!(flat.get("cnc").and_then(Value::as_str), Some(cnc));
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

#[cfg(test)]
mod gcode_save_tests {
    use super::*;

    /// The remembered directory is honoured while it exists — that is the whole point
    /// of persisting it.
    #[test]
    fn an_existing_remembered_directory_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let remembered = dir.path().to_string_lossy().into_owned();
        assert_eq!(resolve_save_directory(Some(&remembered)), dir.path());
    }

    /// The first save (nothing remembered) and a destination that has since been
    /// deleted both land on the host default rather than a path that no longer exists.
    #[test]
    fn a_missing_or_absent_destination_falls_back_to_the_host_default() {
        let host = host_default_save_directory();
        assert_eq!(resolve_save_directory(None), host, "first save");
        assert_eq!(resolve_save_directory(Some("")), host, "blank setting");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        drop(dir); // the folder is gone, as if the user deleted or unmounted it
        assert_eq!(resolve_save_directory(Some(&path)), host, "destination removed");
    }

    /// A file path is not a directory — a stale setting pointing at one must not be
    /// handed to the dialog as a starting folder.
    #[test]
    fn a_remembered_path_that_is_a_file_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory.nc");
        std::fs::write(&file, "G21").unwrap();
        let as_str = file.to_string_lossy().into_owned();
        assert_eq!(resolve_save_directory(Some(&as_str)), host_default_save_directory());
    }

    /// The default name is the board's own, with characters Windows rejects folded to
    /// underscores so the dialog never opens with an unusable suggestion.
    #[test]
    fn the_default_file_stem_is_sanitised() {
        assert_eq!(sanitize_file_stem("panel"), "panel");
        assert_eq!(sanitize_file_stem("panel v2: rev/3"), "panel v2_ rev_3");
        assert_eq!(sanitize_file_stem(r#"a<b>c"d\e|f?g*h"#), "a_b_c_d_e_f_g_h");
        // Windows silently drops trailing dots and spaces from a name.
        assert_eq!(sanitize_file_stem("  board.  "), "board");
    }

    /// The host default must always resolve to something usable, even on a machine
    /// with no Downloads folder — the dialog cannot be handed an empty path.
    #[test]
    fn the_host_default_is_always_a_path() {
        assert!(!host_default_save_directory().as_os_str().is_empty());
    }

    /// A state carrying just enough for the naming rules.
    fn named_board(name: &str) -> AppState {
        let mut app = AppState::new(&UiLaunchData {
            kicad_status: String::new(),
            board_snapshot: None,
        });
        app.board = Some(pcb::BoardSnapshot {
            name: name.to_string(),
            thickness: None,
            bounding_box: None,
            edge_shapes: Vec::new(),
            holes: Vec::new(),
        });
        app
    }

    /// A single-step job is named for the board alone: a `_step1` suffix on the only file
    /// there is would be the step machinery showing through, which is exactly what the
    /// one-step case must not do. Its step name is ignored for the same reason.
    #[test]
    fn one_step_is_named_for_the_board_alone() {
        let app = named_board("panel");
        assert_eq!(app.program_file_name(0, 1, "", "nc"), "panel.nc");
        assert_eq!(app.program_file_name(0, 1, "Drill PTH", "nc"), "panel.nc");
        assert_eq!(app.gcode_default_file_name(), "panel.nc", "the old name is unchanged");
    }

    /// Past one step the file is named for the step, because that is the name the operator
    /// is working from: a stick holding `panel_Drill PTH.nc` says what to load when, and
    /// `panel_step1.nc` does not.
    #[test]
    fn a_named_step_names_its_file() {
        let app = named_board("panel");
        assert_eq!(app.program_file_name(0, 3, "Drill PTH", "nc"), "panel_Drill PTH.nc");
        assert_eq!(app.program_file_name(2, 3, "Route outline", "nc"), "panel_Route outline.nc");
    }

    /// The ordinal is the fallback, and has to survive a name that sanitises away to
    /// nothing — a step called `?` must not produce `panel_.nc`.
    #[test]
    fn an_unnamed_step_falls_back_to_its_ordinal() {
        let app = named_board("panel");
        assert_eq!(app.program_file_name(0, 3, "", "nc"), "panel_step1.nc");
        assert_eq!(app.program_file_name(2, 3, "   ", "nc"), "panel_step3.nc");
        assert_eq!(app.program_file_name(1, 3, "?", "nc"), "panel_step2.nc", "sanitised to nothing");
        assert_eq!(app.program_file_name(1, 3, "...", "nc"), "panel_step2.nc");
    }

    /// The extension comes from the step's own CNC, so a machine that emits Excellon does
    /// not have its output called `.nc`.
    #[test]
    fn the_steps_own_extension_is_honoured() {
        let app = named_board("panel");
        assert_eq!(app.program_file_name(1, 2, "", "drl"), "panel_step2.drl");
        // A profile that somehow carries none still produces a usable name.
        assert_eq!(app.program_file_name(0, 1, "", ""), "panel.nc");
        assert_eq!(app.program_file_name(0, 1, "", ".ngc"), "panel.ngc", "a stray dot is tolerated");
    }

    /// Both halves are sanitised, and by the same rule — the step name is the operator's
    /// free text and is the likelier of the two to carry a slash or a colon.
    #[test]
    fn the_board_and_the_step_are_both_sanitised() {
        let app = named_board("panel v2: rev/3");
        assert_eq!(app.program_file_name(1, 2, "", "nc"), "panel v2_ rev_3_step2.nc");
        assert_eq!(
            app.program_file_name(0, 2, "Drill: PTH/NPTH", "nc"),
            "panel v2_ rev_3_Drill_ PTH_NPTH.nc"
        );
    }
}

#[cfg(test)]
mod job_dock_tests {
    use super::*;

    /// The dock is only offered where an edit can actually change what it shows. The
    /// Job screen already *is* the view, and Logs/About feed nothing into the plan.
    #[test]
    fn only_the_profile_and_inventory_screens_carry_the_dock() {
        for screen in [
            Screen::CncProfiles,
            Screen::FixtureProfiles,
            Screen::MachiningProfiles,
            Screen::ToolsetProfiles,
            Screen::Stock,
            Screen::Catalog,
        ] {
            assert!(screen.shows_pinned_job(), "{:?} should carry the dock", screen.label());
        }
        for screen in [Screen::Job, Screen::Logs, Screen::About] {
            assert!(!screen.shows_pinned_job(), "{:?} should not", screen.label());
        }
    }

    /// A hand-edited or stale settings file cannot produce an unreadable column.
    ///
    /// Floor only. A width wider than the window is *not* clamped here, because the
    /// layout already reserves the screen beside it and hands the dock whatever is left
    /// — so an over-large value resolves to "as wide as it goes" rather than to a number
    /// this code would have to guess at without knowing the window.
    #[test]
    fn the_persisted_dock_width_is_floored_but_not_capped() {
        const { assert!(MIN_JOB_PIN_WIDTH < DEFAULT_JOB_PIN_WIDTH) };
        assert_eq!((-500i64).max(MIN_JOB_PIN_WIDTH), MIN_JOB_PIN_WIDTH);
        assert_eq!(99_999i64.max(MIN_JOB_PIN_WIDTH), 99_999, "no ceiling to hit");
    }
}

#[cfg(test)]
mod readiness_gate_tests {
    use super::*;

    /// The required-primitive list must match the schema's, because it once did not.
    ///
    /// A hand-written copy lived in `machine_required_paths` and named `initialise`,
    /// `change_tool`, `rapid_move` and four more — the names as they were *before*
    /// `PRIMITIVE_RENAMES` migrated every stored profile onto `program_begin`,
    /// `tool_change`, `move_rapid`. Nothing tied the two lists together, so the rename
    /// landed without touching this one and every CNC profile in existence began
    /// reporting seven missing required fields. The readiness gate then refused to
    /// generate for any job at all, with a message that named neither the profile nor the
    /// fields, and the only trace was a line in the log.
    ///
    /// The list is now read from the schema, so this asserts the wiring rather than a
    /// duplicate: if `required_primitives` ever silently returns nothing — an unparsable
    /// schema — the gate would stop checking primitives entirely and nothing else would
    /// notice.
    #[test]
    fn the_required_primitives_come_from_the_schema_and_are_current() {
        let required = crate::gcode::primitive_vars::required_primitives();
        assert!(
            !required.is_empty(),
            "the schema's primitives.required list did not parse, so the gate is no \
             longer checking that a profile can emit anything at all"
        );

        let paths = machine_required_paths();
        for name in required {
            assert!(
                paths.iter().any(|path| path == &format!("primitives.{name}")),
                "the schema requires primitive `{name}` but the readiness gate does not \
                 check for it"
            );
        }

        // The names that caused it, pinned by name: none of these may come back.
        for retired in
            ["initialise", "conclude", "change_tool", "start_spindle", "stop_spindle",
             "rapid_move", "linear_cut", "banner", "line_number"]
        {
            assert!(
                !paths.iter().any(|path| path == &format!("primitives.{retired}")),
                "`{retired}` is a pre-rename primitive name; requiring it means requiring \
                 a field no migrated profile has, which shuts the gate on every job"
            );
        }
    }

    /// A bundled CNC template must satisfy the gate.
    ///
    /// This is the end-to-end version of the test above, and the one that would have
    /// caught the bug on its own: it takes a profile exactly as the application ships it
    /// and asserts the gate finds nothing missing. Whatever the required list is derived
    /// from, a template the application itself provides has to pass it.
    #[test]
    fn every_bundled_cnc_template_satisfies_the_readiness_gate() {
        for (key, yaml) in crate::data::CNC_TEMPLATES {
            let mut value: Value =
                serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{key} parses: {e}"));
            // As loaded: the rename migration runs before anything reads a profile, and
            // it is exactly the step whose output the stale list disagreed with.
            crate::data::normalize_cnc_value(&mut value, std::path::Path::new("template.yaml"));
            // Templates carry no id (one is minted on instantiation), so that alone is
            // expected; nothing else may be.
            let missing: Vec<String> = collect_missing_required(&value, machine_required_paths())
                .into_iter()
                .filter(|path| path != "id")
                .collect();
            assert!(
                missing.is_empty(),
                "bundled template '{key}' would be judged incomplete, so a job using it \
                 could never generate. Missing: {missing:?}"
            );
        }
    }

    fn bare_app() -> AppState {
        AppState::new(&UiLaunchData { kicad_status: String::new(), board_snapshot: None })
    }

    /// A no-go reason becomes a standing diagnostic, and stops being one when it is fixed.
    ///
    /// This is the half the operator actually experiences: before it, a shut gate wrote a
    /// `log::warn!` and set a pill to "Not ready", so the only way to find out what the
    /// application objected to was to open the Logs screen. The banner sits above every
    /// screen, so the reason is now wherever the operator is standing.
    #[test]
    fn a_no_go_reason_is_published_and_withdrawn_with_the_gate() {
        let mut app = bare_app();
        app.set_readiness_errors(&["CNC profile 'MASSO' is missing primitives.drill".to_string()]);

        let published: Vec<&str> = app
            .errors
            .iter()
            .filter(|e| e.domain == READINESS_ERROR_DOMAIN)
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(published, ["CNC profile 'MASSO' is missing primitives.drill"]);

        // The gate opening must take the banner with it, with nothing to dismiss.
        app.set_readiness_errors(&[]);
        assert!(app.errors.iter().all(|e| e.domain != READINESS_ERROR_DOMAIN));
    }

    /// Republishing an unchanged set must not disturb what is on screen.
    ///
    /// The gate is re-evaluated on every mutation, and the entries' ids key the banner's
    /// detail list — so a clear-and-repush would tear down and rebuild the entry an
    /// operator was reading, on every keystroke of an edit elsewhere.
    #[test]
    fn republishing_the_same_reasons_leaves_the_entries_alone() {
        let mut app = bare_app();
        let reasons = vec!["PCB data not loaded".to_string()];

        app.set_readiness_errors(&reasons);
        let first: Vec<String> = app.errors.iter().map(|e| e.id.clone()).collect();
        app.set_readiness_errors(&reasons);
        let second: Vec<String> = app.errors.iter().map(|e| e.id.clone()).collect();
        assert_eq!(first, second, "the same reasons must keep their identity");

        app.set_readiness_errors(&["PCB data not loaded".to_string(), "Open contours detected".to_string()]);
        assert_eq!(
            app.errors.iter().filter(|e| e.domain == READINESS_ERROR_DOMAIN).count(),
            2,
            "a changed set does get republished"
        );
    }

    /// The gate must not read its own output back as a reason to stay shut.
    ///
    /// Every readiness entry is `is_error`, and a blocking config error is itself a no-go
    /// reason — so counting them would close the loop on itself: the gate shuts, publishes
    /// why, sees its own publication as a blocking error, and stays shut however much the
    /// operator fixes. Exactly the trap `GENERATION_ERROR_DOMAIN` is excluded to avoid,
    /// one turn tighter.
    #[test]
    fn readiness_diagnostics_cannot_hold_the_gate_shut_by_themselves() {
        let mut app = bare_app();
        app.set_readiness_errors(&["Referenced CNC profile is missing".to_string()]);
        assert!(
            app.errors.iter().any(|e| e.is_error),
            "the entries are errors — which is the whole hazard being guarded against"
        );
        assert!(
            !has_blocking_config_error(&app),
            "the gate counted its own no-go reasons as a blocking error, so no fix could              ever reopen it"
        );
    }
}
