use dioxus::prelude::*;
use rfd::FileDialog;
use std::fs;
use std::path::Path;

use crate::units::{FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed, RotationalSpeedUnit};
use super::super::model::*;

#[component]
pub fn SetupScreen(state: Signal<UiState>, boot: UiLaunchData) -> Element {
    let snapshot = state.read().clone();
    let library_profiles = cnc_profile_library();
    let mut selected_library_profile = use_signal(|| {
        library_profiles
            .first()
            .map(|p| p.key.clone())
            .unwrap_or_default()
    });
    let mut import_feedback = use_signal(String::new);
    let board_snapshot_summary = snapshot.board.as_ref().map(|board| {
        let bbox_label = if board.bounding_box.is_some() {
            "available"
        } else {
            "missing"
        };
        format!(
            "Board snapshot: bbox {bbox_label}, edge shapes {}, holes {}",
            board.edge_shapes.len(),
            board.holes.len()
        )
    });

    rsx! {
        div { class: "screen split",
            section { class: "panel fixed",
                h3 { "Setup" }

                div { class: "field",
                    label { "Unit System" }
                    select {
                        value: snapshot.unit_system.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.unit_system = if v == "imperial" {
                                        UnitSystem::Imperial
                                    } else {
                                        UnitSystem::Metric
                                    };
                                });
                        },
                        option { value: "metric", "Metric" }
                        option { value: "imperial", "Imperial" }
                    }
                }

                div { class: "field",
                    label { "Theme" }
                    select {
                        value: snapshot.theme.as_str(),
                        onchange: move |evt| {
                            let v = evt.value();
                            state
                                .with_mut(|s| {
                                    s.theme = if v == "light" { Theme::Light } else { Theme::Dark };
                                });
                        },
                        option { value: "light", "Light" }
                        option { value: "dark", "Dark" }
                    }
                }

                div { class: "diagnostics",
                    h4 { "Runtime Diagnostics" }
                    p { class: "diag-status", "{boot.kicad_status}" }
                    p { class: "diag-status", "{boot.env_summary}" }
                    if let Some(summary) = board_snapshot_summary.as_ref() {
                        p { class: "diag-status", "{summary}" }
                    } else {
                        p { class: "diag-status", "Board snapshot: unavailable" }
                    }
                }
            }

            section { class: "panel grow",
                div { class: "panel",
                    h3 { "CNC profile management" }
                    p { "Add a CNC profile from library or import a CNC profile from file." }

                    div { class: "field",
                        label { "Add CNC profile from library" }
                        select {
                            value: selected_library_profile.read().clone(),
                            onchange: move |evt| selected_library_profile.set(evt.value()),
                            for profile in library_profiles.iter() {
                                option { value: profile.key.clone(), "{profile.name}" }
                            }
                        }
                        button {
                            class: "btn btn-primary",
                            onclick: {
                                let profiles = library_profiles.clone();
                                move |_| {
                                    let key = selected_library_profile.read().clone();
                                    let selected = profiles.iter().find(|p| p.key == key).cloned();
                                    if let Some(profile) = selected {
                                        state.with_mut(|s| s.add_machine_profile(profile.machine));
                                        import_feedback.set("CNC profile added from library".to_string());
                                    } else {
                                        import_feedback
                                            .set("Unable to add CNC profile from library".to_string());
                                    }
                                }
                            },
                            "Add CNC profile"
                        }
                    }

                    div { class: "field",
                        label { "Import CNC profile" }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                let picked = FileDialog::new()
                                    .set_title("Import CNC profile")
                                    .add_filter("CNC profile YAML", &["yaml", "yml"])
                                    .pick_file();

                                let Some(path) = picked else {
                                    import_feedback.set("Import canceled".to_string());
                                    return;
                                };

                                let file_name = path
                                    .file_name()
                                    .and_then(|f| f.to_str())
                                    .unwrap_or_default()
                                    .to_ascii_lowercase();
                                let valid_name = file_name.ends_with(".cnc-profile.yaml")
                                    || file_name.ends_with(".cnc-profile.yml");
                                if !valid_name {
                                    import_feedback
                                        .set(
                                            "CNC profile import failed: file name must end with .cnc-profile.yaml or .cnc-profile.yml"
                                                .to_string(),
                                        );
                                    return;
                                }
                                let text = match fs::read_to_string(&path) {
                                    Ok(text) => text,
                                    Err(_) => {
                                        import_feedback
                                            .set("CNC profile import failed: file not readable".to_string());
                                        return;
                                    }
                                };
                                let path_str = path.to_string_lossy().to_string();
                                let profile = match parse_machine_profile_yaml(&text, &path_str) {
                                    Some(profile) => profile,
                                    None => {
                                        import_feedback
                                            .set(
                                                "CNC profile import failed: file is missing required machine fields"
                                                    .to_string(),
                                            );
                                        return;
                                    }
                                };
                                state.with_mut(|s| s.add_machine_profile(profile));
                                import_feedback.set("CNC profile imported and selected".to_string());
                            },
                            "Import CNC profile"
                        }
                    }
                }

                div { class: "panel",
                    div { class: "panel-header",
                        h3 { "Tools catalog management" }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                let picked = FileDialog::new()
                                    .set_title("Import catalog")
                                    .add_filter("Catalog YAML", &["yaml", "yml"])
                                    .pick_file();

                                let Some(path) = picked else {
                                    import_feedback.set("Catalog import canceled".to_string());
                                    return;
                                };

                                let text = match fs::read_to_string(&path) {
                                    Ok(text) => text,
                                    Err(_) => {
                                        import_feedback
                                            .set("Catalog import failed: file not readable".to_string());
                                        return;
                                    }
                                };
                                let stem = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("catalog")
                                    .to_string();
                                state
                                    .with_mut(|s| {
                                        match s.import_catalog_text(&stem, &text) {
                                            Ok(name) => {
                                                import_feedback.set(format!("Catalog '{}' imported", name));
                                            }
                                            Err(msg) => {
                                                import_feedback.set(msg);
                                            }
                                        }
                                    });
                            },
                            "Import catalog"
                        }
                    }

                    div { class: "table-wrap",
                        table {
                            thead {
                                tr {
                                    th { "Catalog" }
                                    th { "Type" }
                                    th { "Sections" }
                                    th { "Actions" }
                                }
                            }
                            tbody {
                                for catalog in snapshot.catalogs.iter() {
                                    tr {
                                        td { "{catalog.name}" }
                                        td {
                                            if catalog.built_in {
                                                "Built-in"
                                            } else {
                                                "Imported"
                                            }
                                        }
                                        td { "{catalog.sections.len()}" }
                                        td {
                                            if catalog.built_in {
                                                span { class: "status-chip status-new",
                                                    "Protected"
                                                }
                                            } else {
                                                button {
                                                    class: "btn btn-danger btn-small",
                                                    onclick: {
                                                        let key = catalog.key.clone();
                                                        move |_| {
                                                            state
                                                                .with_mut(|s| {
                                                                    match s.remove_catalog(&key) {
                                                                        Ok(_) => import_feedback.set("Catalog deleted".to_string()),
                                                                        Err(msg) => import_feedback.set(msg),
                                                                    }
                                                                });
                                                        }
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if !import_feedback.read().is_empty() {
                    p { class: "diag-status", "{import_feedback}" }
                }
            }
        }
    }
}

fn parse_machine_profile_yaml(text: &str, source_path: &str) -> Option<MachineProfile> {
    let root: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let machine = root.get("machine")?;

    let fixture = machine.get("fixture_plate")?;
    let x = Length::from_string(fixture.get("x")?.as_str()?, Some(LengthUnit::Mm))
        .ok()?
        .as_mm() as u32;
    let y = Length::from_string(fixture.get("y")?.as_str()?, Some(LengthUnit::Mm))
        .ok()?
        .as_mm() as u32;

    let max_feed_rate_mm_per_min = machine
        .get("max_feed_rate")
        .and_then(|v| v.as_str())
        .and_then(|s| FeedRate::from_string(s, Some(FeedRateUnit::MmPerMin)).ok())
        .map(|f| f.as_mm_per_min() as u32)
        .unwrap_or(2000);

    let spindle_min_rpm = RotationalSpeed::from_string(
        machine.get("spindle_rpm_min")?.as_str()?,
        Some(RotationalSpeedUnit::Rpm),
    )
    .ok()?
    .as_rpm() as u32;
    let spindle_max_rpm = RotationalSpeed::from_string(
        machine.get("spindle_rpm_max")?.as_str()?,
        Some(RotationalSpeedUnit::Rpm),
    )
    .ok()?
    .as_rpm() as u32;
    let atc_slot_count = machine.get("atc_slot_count")?.as_i64()? as u8;

    let origin = machine.get("origin");
    let origin_x0 = origin
        .and_then(|o| o.get("x0"))
        .and_then(|v| v.as_str())
        .unwrap_or("Left")
        .to_string();
    let origin_y0 = origin
        .and_then(|o| o.get("y0"))
        .and_then(|v| v.as_str())
        .unwrap_or("Front")
        .to_string();

    let scaling = machine.get("scaling");
    let scaling_x = scaling
        .and_then(|s| s.get("x"))
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0) as f32;
    let scaling_y = scaling
        .and_then(|s| s.get("y"))
        .and_then(|v| v.as_f64())
        .unwrap_or(100.0) as f32;

    let line_num = machine.get("line_numbering");
    let line_numbering_enabled = line_num
        .and_then(|l| l.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let line_numbering_increment = line_num
        .and_then(|l| l.get("increment"))
        .and_then(|v| v.as_i64())
        .unwrap_or(10) as u32;

    let def = MachineProfile::default();

    let trim_nl = |s: &str| s.trim_end_matches('\n').to_string();

    let gcode_header = root
        .get("header")
        .and_then(|v| v.as_str())
        .map(trim_nl)
        .unwrap_or(def.gcode_header.clone());
    let gcode_footer = root
        .get("footer")
        .and_then(|v| v.as_str())
        .map(trim_nl)
        .unwrap_or(def.gcode_footer.clone());

    let drill = root.get("drill");
    let str_field = |section: Option<&serde_yaml::Value>, key: &str, fallback: &str| -> String {
        section
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('\n').to_string())
            .unwrap_or_else(|| fallback.to_string())
    };
    let drill_first_move      = str_field(drill, "first_move",      &def.drill_first_move);
    let drill_cycle_mode_last = str_field(drill, "cycle_mode_last", &def.drill_cycle_mode_last);
    let drill_cycle_mode_series = str_field(drill, "cycle_mode_series", &def.drill_cycle_mode_series);
    let drill_cycle_start     = str_field(drill, "cycle_start",     &def.drill_cycle_start);
    let drill_next_hole       = str_field(drill, "next_hole",       &def.drill_next_hole);
    let drill_cycle_cancel    = str_field(drill, "cycle_cancel",    &def.drill_cycle_cancel);

    let route = root.get("route");
    let route_plunge_and_offset = str_field(route, "plunge_and_offset", &def.route_plunge_and_offset);
    let route_arc_up   = str_field(route, "arc_up",  &def.route_arc_up);
    let route_arc_down = str_field(route, "arc_down", &def.route_arc_down);
    let route_retract  = str_field(route, "retract",  &def.route_retract);

    let tc = root.get("tool_change");
    let tool_change_manual_prompt = str_field(tc, "manual_prompt", &def.tool_change_manual_prompt);
    let tool_change_command       = str_field(tc, "command",       &def.tool_change_command);

    let id_stem = Path::new(source_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("machine")
        .to_string();

    let display_name = root
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Imported {id_stem}"));

    Some(MachineProfile {
        id: format!("import-{}", slug(&id_stem)),
        name: display_name,
        fixture_plate_max_x: x,
        fixture_plate_max_y: y,
        max_feed_rate_mm_per_min,
        spindle_min_rpm,
        spindle_max_rpm,
        atc_slot_count,
        origin_x0,
        origin_y0,
        scaling_x,
        scaling_y,
        line_numbering_enabled,
        line_numbering_increment,
        gcode_header,
        gcode_footer,
        drill_first_move,
        drill_cycle_mode_last,
        drill_cycle_mode_series,
        drill_cycle_start,
        drill_next_hole,
        drill_cycle_cancel,
        route_plunge_and_offset,
        route_arc_up,
        route_arc_down,
        route_retract,
        tool_change_manual_prompt,
        tool_change_command,
    })
}

#[derive(Clone)]
struct LibraryProfile {
    key: String,
    name: String,
    machine: MachineProfile,
}

fn cnc_profile_library() -> Vec<LibraryProfile> {
    vec![
        LibraryProfile {
            key: "masso-g3-compact".to_string(),
            name: "Masso G3 Compact".to_string(),
            machine: MachineProfile {
                id: "library-masso-g3-compact".to_string(),
                name: "Masso G3 Compact".to_string(),
                fixture_plate_max_x: 300,
                fixture_plate_max_y: 200,
                max_feed_rate_mm_per_min: 2000,
                spindle_min_rpm: 3000,
                spindle_max_rpm: 24000,
                atc_slot_count: 8,
                ..MachineProfile::default()
            },
        },
        LibraryProfile {
            key: "masso-g3-manual".to_string(),
            name: "Masso G3 Manual Tool Change".to_string(),
            machine: MachineProfile {
                id: "library-masso-g3-manual".to_string(),
                name: "Masso G3 Manual Tool Change".to_string(),
                fixture_plate_max_x: 250,
                fixture_plate_max_y: 180,
                max_feed_rate_mm_per_min: 2000,
                spindle_min_rpm: 2500,
                spindle_max_rpm: 18000,
                atc_slot_count: 0,
                ..MachineProfile::default()
            },
        },
        LibraryProfile {
            key: "router-gantry-pro".to_string(),
            name: "Router Gantry Pro".to_string(),
            machine: MachineProfile {
                id: "library-router-gantry-pro".to_string(),
                name: "Router Gantry Pro".to_string(),
                fixture_plate_max_x: 500,
                fixture_plate_max_y: 350,
                max_feed_rate_mm_per_min: 3000,
                spindle_min_rpm: 4000,
                spindle_max_rpm: 22000,
                atc_slot_count: 10,
                ..MachineProfile::default()
            },
        },
    ]
}

fn slug(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        }
    }
    if out.is_empty() { "machine".to_string() } else { out }
}
