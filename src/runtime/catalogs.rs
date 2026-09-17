/// Keeps the user's on-disk copy of every *bundled* catalog (`kyocera.yaml`,
/// `unionfab.yaml`, `generic.yaml`) in step with the copy embedded in this binary.
///
/// The binary is the reference; the file on disk is a convenience — it exists only
/// so a bundled catalog loads through the same `CatalogManager::load_dir` path as
/// everything else, not as a place to hand-edit. When a new build embeds a tool the
/// disk copy doesn't have yet (or any other content change), the disk copy is
/// replaced outright — including a hand edit, which is indistinguishable here from
/// staleness — the same way `data::schema_export`'s `write_if_changed` keeps the
/// published schema docs in step with the schemas compiled into the app. A
/// user-imported catalog — anything in the directory that isn't one of these three
/// names — is never touched here; only files present in [`default_catalogs`] are
/// ever candidates.
///
/// The comparison is against the *canonicalised* embedded text (run through the
/// same enrichment `canonicalize_catalog_text` applies — inject missing `id`/`sku`/
/// `schema`/`point_angle`/`z_min_depth`), not the raw embedded source: the bundled
/// catalogs are authored without those fields, so a freshly-synced file never
/// matches its own raw source again once they're filled in — comparing against raw
/// text would make every disk copy look perpetually stale and rewrite it on every
/// single startup.
///
/// Returns how many of the built-in files were (re)written, so a test can assert
/// "nothing to do" on a second call the same way `schema_export::ensure_schema_files`
/// does.
fn sync_builtin_catalogs(dir: &std::path::Path) -> usize {
    let mut written = 0;

    for (name, embedded) in default_catalogs() {
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("catalog");

        let canonical = match canonicalize_catalog_text(embedded, stem) {
            Ok(text) => text,
            Err(e) => {
                warn!("Could not canonicalize bundled catalog '{name}': {e}");
                continue;
            }
        };

        let dest = dir.join(name);
        let up_to_date =
            std::fs::read_to_string(&dest).is_ok_and(|existing| existing == canonical);
        if up_to_date {
            continue;
        }

        match std::fs::write(&dest, &canonical) {
            Ok(()) => {
                info!(
                    "Synced bundled catalog '{}' to the version embedded in this build: {}",
                    name,
                    dest.display()
                );
                written += 1;
            }
            Err(e) => warn!("Could not write catalog '{}': {e}", dest.display()),
        }
    }

    written
}

fn load_catalog_index() -> Vec<CatalogStockCatalog> {
    let mut source_catalogs: Vec<(String, Catalog, bool)> = Vec::new();

    if let Ok(dir) = catalog_dir() {
        sync_builtin_catalogs(&dir);
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
mod bundled_catalog_sync_tests {
    use super::*;
    use tempfile::tempdir;

    /// The bug this exists to prevent: a tool added to `assets/catalogs/generic.yaml`
    /// in a new build never reached a user whose catalog directory already held a
    /// copy seeded by an older build — `ensure_default_files`' "existing files are
    /// preserved" policy left it frozen at whatever it looked like the first time it
    /// was ever seeded. Simulating that here: seed once, then append a tool to what
    /// the "embedded" source would be understood as (by writing a stale disk copy
    /// missing it) and confirm a resync replaces it with the current, complete form.
    #[test]
    fn a_stale_disk_copy_is_replaced_with_the_currently_embedded_catalog() {
        let dir = tempdir().unwrap();

        assert!(sync_builtin_catalogs(dir.path()) > 0, "first run seeds every bundled catalog");

        let generic = dir.path().join("generic.yaml");
        let current = std::fs::read_to_string(&generic).unwrap();
        std::fs::write(&generic, "# a build from before this tool existed\nname: Old\n").unwrap();

        assert_eq!(
            sync_builtin_catalogs(dir.path()),
            1,
            "only the one file that drifted from the binary is rewritten"
        );
        assert_eq!(
            std::fs::read_to_string(&generic).unwrap(),
            current,
            "restored to what this build embeds, tools included"
        );
    }

    /// The steady-state case a bare "seed if missing" gate can't tell apart from the
    /// one above: nothing changed, so nothing should be rewritten. Confirmed by
    /// return value rather than a modification-time check, the same way
    /// `schema_export::ensure_schema_files`'s own idempotency test is written.
    #[test]
    fn an_up_to_date_catalog_is_not_rewritten() {
        let dir = tempdir().unwrap();

        assert!(sync_builtin_catalogs(dir.path()) > 0, "first launch writes");
        assert_eq!(sync_builtin_catalogs(dir.path()), 0, "second launch has nothing to do");
    }

    /// A catalog a user dropped into the directory under a name that isn't one of
    /// the three bundled ones is a real import, not a stale copy — it must never be
    /// touched by the built-in sync.
    #[test]
    fn a_user_imported_catalog_is_left_alone() {
        let dir = tempdir().unwrap();
        let imported = dir.path().join("my_import.yaml");
        std::fs::write(&imported, "name: Mine\nsections: []\n").unwrap();

        sync_builtin_catalogs(dir.path());

        assert_eq!(std::fs::read_to_string(&imported).unwrap(), "name: Mine\nsections: []\n");
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
