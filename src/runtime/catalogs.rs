fn load_catalog_index() -> Vec<CatalogStockCatalog> {
    let mut source_catalogs: Vec<(String, Catalog, bool)> = Vec::new();

    if let Ok(dir) = catalog_dir() {
        ensure_default_files(&dir, default_catalogs(), "catalog", |path| {
            if let Err(e) = backfill_catalog_fields(path) {
                warn!("Could not backfill catalog '{}': {e}", path.display());
            }
        });
    }

    // The bundled catalogs are seeded into the user's catalog dir and then loaded
    // back from disk like any other file, so identify them by filename stem: those
    // are protected (built-in), everything else in the directory is a user import
    // and may be deleted.
    let builtin_stems: std::collections::HashSet<String> = default_catalogs()
        .iter()
        .filter_map(|(name, _)| {
            std::path::Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();

    if let (Ok(mut manager), Ok(dir)) = (CatalogManager::new(), catalog_dir()) {
        let _ = manager.load_dir(&dir);
        source_catalogs = manager
            .catalogs()
            .map(|(stem, catalog)| {
                let built_in = builtin_stems.contains(stem);
                (stem.to_string(), catalog.clone(), built_in)
            })
            .collect();
    }

    if source_catalogs.is_empty() {
        let sources = [
            ("kyocera".to_string(), include_str!("../../assets/catalogs/kyocera.yaml")),
            ("unionfab".to_string(), include_str!("../../assets/catalogs/unionfab.yaml")),
            ("generic".to_string(), include_str!("../../assets/catalogs/generic.yaml")),
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
