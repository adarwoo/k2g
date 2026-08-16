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
            // The diameter of a V-bit is its tip, not its shank.
            diameter: Length::from_mm(0.1),
            flute_length: Some(Length::from_mm(12.0)),
            sku: Some("test-vbit".into()),
            point_angle: units::Angle::from_degrees(30.0),
            z_min_depth: Length::from_mm(0.0),
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

    /// The bug this exists to prevent: a V-bit's cutting geometry was dropped at every
    /// layer between the catalogue file and the stock tool — four of them — and
    /// `pick_engraver` chooses a bit *by* it, so every V-bit ever added was unusable and
    /// the engrave operation planned nothing. The whole chain is walked here because no
    /// single layer was at fault.
    #[test]
    fn a_v_bit_keeps_its_cutting_geometry_from_the_catalogue_into_stock() {
        let catalog = catalog_with(vbit_entry());
        let projected = catalog_to_stock_catalog("test", "Test", &catalog, true);

        let listed = &projected.sections[0].tools[0];
        assert_eq!(listed.diameter, Length::from_mm(0.1), "the tip, in the picker's list");
        assert_eq!(listed.flute_length, Some(Length::from_mm(12.0)));
        assert_eq!(listed.z_min_depth, Some(Length::from_mm(0.0)));

        let mut app = AppState::new(&UiLaunchData {
            kicad_status: String::new(),
            board_snapshot: None,
            copper: Default::default(),
        });
        app.catalogs = vec![projected];
        let added = app.build_catalog_tool_additions(
            &[app.catalogs[0].sections[0].tools[0].key.clone()],
            false,
        );

        assert_eq!(added.len(), 1, "one tool selected, one added");
        assert_eq!(
            added[0].diameter,
            Length::from_mm(0.1),
            "and it arrives in stock with the tip it will be chosen for"
        );
        assert_eq!(added[0].flute_length, Some(Length::from_mm(12.0)));
        assert_eq!(added[0].z_min_depth, Some(Length::from_mm(0.0)));
    }

    /// The rating a bit is held to survives the same trip. It is the other half of what
    /// `pick_engraver` decides on, and a bit that lost it would be accepted at any depth
    /// however shallow.
    #[test]
    fn the_depth_rating_survives_the_trip_too() {
        let mut deep = vbit_entry();
        deep.z_min_depth = Length::from_mm(0.05);

        let projected = catalog_to_stock_catalog("test", "Test", &catalog_with(deep), true);
        assert_eq!(
            projected.sections[0].tools[0].z_min_depth,
            Some(Length::from_mm(0.05))
        );
    }

    /// An app holding one catalogue, and the key of its only tool.
    fn app_with_one_catalog_tool() -> (AppState, String) {
        let projected = catalog_to_stock_catalog("test", "Test", &catalog_with(vbit_entry()), true);
        let mut app = AppState::new(&UiLaunchData {
            kicad_status: String::new(),
            board_snapshot: None,
            copper: Default::default(),
        });
        app.catalogs = vec![projected];
        let key = app.catalogs[0].sections[0].tools[0].key.clone();
        (app, key)
    }

    /// The bulk picker declines to hand back a second copy of what is already owned —
    /// selecting a whole section should not duplicate a rack's worth of tools.
    #[test]
    fn the_bulk_add_skips_a_tool_already_in_stock() {
        let (mut app, key) = app_with_one_catalog_tool();

        let first = app.build_catalog_tool_additions(&[key.clone()], false);
        assert_eq!(first.len(), 1, "nothing owned yet, so it is added");
        app.tools = first;

        let again = app.build_catalog_tool_additions(&[key], false);
        assert!(again.is_empty(), "and the second time it is recognised");
    }

    /// Naming one tool and pressing Add is a specific request, so it is honoured — and
    /// the copy is named apart, because a tool's name is what identifies it in the rack
    /// picker and the tooling plan.
    #[test]
    fn a_single_add_takes_a_second_copy_and_names_it_apart() {
        let (mut app, key) = app_with_one_catalog_tool();

        let first = app.build_catalog_tool_additions(&[key.clone()], true);
        let original = first[0].composite_name.clone();
        app.tools = first;

        let second = app.build_catalog_tool_additions(&[key.clone()], true);
        assert_eq!(second.len(), 1, "asked for, so added");
        assert_eq!(second[0].composite_name, format!("{original} (2)"));
        app.tools.extend(second);

        let third = app.build_catalog_tool_additions(&[key], true);
        assert_eq!(
            third[0].composite_name,
            format!("{original} (3)"),
            "and it keeps counting rather than colliding with the second"
        );
    }

    /// The suffix appears only when the name is taken. An ordinary add — the
    /// overwhelmingly common case — must look exactly as it always did.
    #[test]
    fn a_first_add_is_not_renamed() {
        let (app, key) = app_with_one_catalog_tool();
        let added = app.build_catalog_tool_additions(&[key], true);
        assert!(
            !added[0].composite_name.ends_with("(2)"),
            "got {}",
            added[0].composite_name
        );
    }

    /// Origin identity, not the current name: renaming a tool in stock must not make
    /// the catalogue entry it came from look unowned.
    #[test]
    fn a_renamed_stock_tool_is_still_recognised_as_the_catalogue_entry() {
        let (mut app, key) = app_with_one_catalog_tool();
        let mut added = app.build_catalog_tool_additions(&[key.clone()], false);
        added[0].name = "The one in the little drawer".to_string();
        app.tools = added;

        let tool = app.catalogs[0].sections[0].tools[0].clone();
        assert!(
            app.catalog_tool_in_stock(&tool, &[]),
            "the SKU still says where it came from"
        );
        assert!(app.build_catalog_tool_additions(&[key], false).is_empty());
    }
}
