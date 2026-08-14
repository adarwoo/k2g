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
                tip_diameter: core.tip_diameter,
                z_min_depth: core.z_min_depth,
                flute_length: core.flute_length,
                table_feed: core.table_feed,
                z_feed: core.z_feed,
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

#[cfg(test)]
mod catalog_projection_tests {
    use super::*;
    use crate::data::model::catalog::{CatalogSection, ToolEntry, ToolType};

    fn vbit_entry() -> ToolEntry {
        ToolEntry {
            id: "id".into(),
            tool_type: ToolType::Vbit,
            diameter: Length::from_mm(3.175),
            flute_length: Some(Length::from_mm(12.0)),
            sku: Some("test-vbit".into()),
            point_angle: units::Angle::from_degrees(30.0),
            z_min_depth: Length::from_mm(0.0),
            tip_diameter: Some(Length::from_mm(0.1)),
            spindle_rpm: None,
            z_feed: None,
            table_feed: None,
            max_hits: None,
            notes: None,
        }
    }

    fn catalog_with(entry: ToolEntry) -> Catalog {
        Catalog {
            name: "Test".into(),
            description: None,
            sections: vec![CatalogSection {
                name: "V-bits".into(),
                default_flute_length_unit: None,
                description: None,
                tools: vec![entry],
            }],
        }
    }

    /// The bug this exists to prevent: the tip diameter was dropped at every layer between
    /// the catalogue file and the stock tool — four of them — and `pick_engraver` chooses
    /// a bit *by* its tip, so every V-bit ever added was unusable and the engrave
    /// operation planned nothing. The whole chain is walked here because no single layer
    /// was at fault.
    #[test]
    fn a_v_bit_keeps_its_tip_all_the_way_from_the_catalogue_into_stock() {
        let catalog = catalog_with(vbit_entry());
        let projected = catalog_to_stock_catalog("test", "Test", &catalog, true);

        let listed = &projected.sections[0].tools[0];
        assert_eq!(listed.tip_diameter, Some(Length::from_mm(0.1)), "the picker's own list");
        assert_eq!(listed.flute_length, Some(Length::from_mm(12.0)));
        assert_eq!(listed.z_min_depth, Some(Length::from_mm(0.0)));

        let mut app = AppState::new(&UiLaunchData {
            kicad_status: String::new(),
            board_snapshot: None,
            copper: Default::default(),
        });
        app.catalogs = vec![projected];
        let added = app.build_catalog_tool_additions(&[app.catalogs[0].sections[0].tools[0]
            .key
            .clone()]);

        assert_eq!(added.len(), 1, "one tool selected, one added");
        assert_eq!(
            added[0].tip_diameter,
            Some(Length::from_mm(0.1)),
            "and it arrives in stock with the tip it was chosen for"
        );
        assert_eq!(added[0].flute_length, Some(Length::from_mm(12.0)));
    }

    /// A tool with no tip stays `None` rather than defaulting to zero. A zero tip would
    /// let `engrave_depth_mm` report that any width is reachable at some depth, which is a
    /// confident wrong answer where absence is an honest one.
    #[test]
    fn a_tool_without_a_tip_does_not_acquire_one() {
        let mut drill = vbit_entry();
        drill.tool_type = ToolType::Drillbit;
        drill.tip_diameter = None;

        let projected = catalog_to_stock_catalog("test", "Test", &catalog_with(drill), true);
        assert_eq!(projected.sections[0].tools[0].tip_diameter, None);
    }
}
