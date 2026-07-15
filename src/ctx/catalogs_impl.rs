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
            ("kyocera".to_string(), include_str!("../../resources/catalogs/kyocera.yaml")),
            ("unionfab".to_string(), include_str!("../../resources/catalogs/unionfab.yaml")),
            ("generic".to_string(), include_str!("../../resources/catalogs/generic.yaml")),
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
