use dioxus::prelude::*;
use std::path::Path;

use crate::units::{FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed, RotationalSpeedUnit};
use super::super::model::*;
use super::setup_sections::{CatalogManagementPanel, GeneralSettingsPanel, MachineProfilesPanel, SetupSidebar, SetupTab};

#[component]
pub fn SetupScreen(state: Signal<UiState>, boot: UiLaunchData) -> Element {
    let snapshot = state.read().clone();
    let library_profiles = cnc_profile_library();
    let selected_library_profile = use_signal(|| {
        library_profiles
            .first()
            .map(|p| p.key.clone())
            .unwrap_or_default()
    });
    let import_feedback = use_signal(String::new);
    let active_tab = use_signal(|| SetupTab::Cnc);
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
        div { class: "screen setup-shell",
            SetupSidebar { active_tab }

            div { class: "setup-main",
                match *active_tab.read() {
                    SetupTab::General => rsx! {
                        GeneralSettingsPanel {
                            state,
                            kicad_status: boot.kicad_status.clone(),
                            board_snapshot_summary: board_snapshot_summary.clone(),
                        }
                    },
                    SetupTab::Cnc => rsx! {
                        MachineProfilesPanel { state, selected_library_profile, import_feedback }
                    },
                    SetupTab::Catalogs => rsx! {
                        CatalogManagementPanel { state, import_feedback }
                    },
                }
            }
        }
    }
}

pub(super) fn parse_machine_profile_yaml(text: &str, source_path: &str) -> Option<MachineProfile> {
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
        built_in: false,
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
pub(super) struct LibraryProfile {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) machine: MachineProfile,
}

pub(super) fn cnc_profile_library() -> Vec<LibraryProfile> {
    vec![
        LibraryProfile {
            key: "masso-g3-compact".to_string(),
            name: "Masso G3 Compact".to_string(),
            machine: MachineProfile {
                id: "library-masso-g3-compact".to_string(),
                name: "Masso G3 Compact".to_string(),
                built_in: true,
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
                built_in: true,
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
                built_in: true,
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
