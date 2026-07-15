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

        Self {
            app,
            cli_args: boot.cli_args.clone(),
            stitched_board_data,
            kicad_status: boot.kicad_status.clone(),
            issues: vec![],
            status,
            catalogs_loaded: false,
        }
    }

    fn sync_from_app_state(&mut self, state: &AppState) {
        let mut next_app = state.clone();

        // Keep context as the source of truth for lazily-loaded catalogs.
        if self.catalogs_loaded && !self.app.catalogs.is_empty() && next_app.catalogs.is_empty() {
            next_app.catalogs = self.app.catalogs.clone();
        }

        self.app = next_app;

        self.stitched_board_data = self.app.board.as_ref().map(|board| {
            let stitched = stitch_edge_shapes(&board.edge_shapes);
            StitchedBoardData {
                contour_count: stitched.contours.len(),
                error_count: stitched.errors.len(),
                errors: stitched.errors,
            }
        });

        if !self.app.catalogs.is_empty() {
            self.catalogs_loaded = true;
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
