use dioxus::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use crate::units::{FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed, RotationalSpeedUnit};
use super::super::model::*;
use super::setup_sections::{CatalogManagementPanel, GeneralSettingsPanel, MachineProfilesPanel, SetupSidebar, SetupTab};

const CNC_SCHEMA_TEXT: &str = include_str!("../../../resources/schemas/cnc.yaml");

pub(super) fn cnc_schema_required_paths() -> &'static BTreeSet<String> {
    static REQUIRED: OnceLock<BTreeSet<String>> = OnceLock::new();
    REQUIRED.get_or_init(|| parse_cnc_required_paths(CNC_SCHEMA_TEXT))
}

pub(super) fn cnc_required_field_label(key: &str) -> Option<&'static str> {
    match key {
        "machine.fixture_plate.x" => Some("Fixture X"),
        "machine.fixture_plate.y" => Some("Fixture Y"),
        "machine.max_feed_rate" => Some("Max feed rate"),
        "machine.spindle_rpm_min" => Some("Spindle min"),
        "machine.spindle_rpm_max" => Some("Spindle max"),
        "machine.atc_slot_count" => Some("ATC slots"),
        "machine.origin.x0" => Some("X axis origin"),
        "machine.origin.y0" => Some("Y axis origin"),
        "machine.scaling.x" => Some("X scale"),
        "machine.scaling.y" => Some("Y scale"),
        "machine.line_numbering_increment" => Some("Line numbering increment"),
        _ => None,
    }
}

fn parse_cnc_required_paths(schema_text: &str) -> BTreeSet<String> {
    let schema: serde_yaml::Value = match serde_yaml::from_str(schema_text) {
        Ok(value) => value,
        Err(_) => return BTreeSet::new(),
    };

    let mut out = BTreeSet::new();
    collect_required_paths(&schema, None, &mut out);
    out
}

fn collect_required_paths(
    node: &serde_yaml::Value,
    prefix: Option<&str>,
    out: &mut BTreeSet<String>,
) {
    let Some(required) = node.get("required").and_then(|v| v.as_sequence()) else {
        return;
    };

    let properties = node.get("properties").and_then(|v| v.as_mapping());

    for required_name in required.iter().filter_map(|v| v.as_str()) {
        let path = match prefix {
            Some(parent) if !parent.is_empty() => format!("{}.{}", parent, required_name),
            _ => required_name.to_string(),
        };
        out.insert(path.clone());

        let Some(props) = properties else {
            continue;
        };

        let key = serde_yaml::Value::String(required_name.to_string());
        if let Some(child) = props.get(&key) {
            collect_required_paths(child, Some(&path), out);
        }
    }
}

const GENMITSU_3018_TEMPLATE: &str = include_str!("../../../resources/cnc_templates/genmitsu_3018.yaml");
const MASSO_G3_NO_ATC_TEMPLATE: &str = include_str!("../../../resources/cnc_templates/masso_g3_no_atc.yaml");
const MASSO_G3_WITH_ATC_TEMPLATE: &str = include_str!("../../../resources/cnc_templates/masso_g3_with_atc.yaml");

#[component]
pub fn SetupScreen(state: Signal<crate::ctx::AppCtx>, boot: UiLaunchData) -> Element {
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

    let line_numbering_increment: u16 = machine
        .get("line_numbering_increment")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            machine
                .get("line_numbering")
                .and_then(|l| l.get("increment"))
                .and_then(|v| v.as_i64())
        })
        .unwrap_or(10)
        .max(0) as u16;

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
        pending_required_fields: BTreeSet::new(),
        usable: true,
    })
}

#[derive(Clone)]
pub(super) struct LibraryProfile {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) machine: MachineProfile,
}

pub(super) fn cnc_profile_library() -> Vec<LibraryProfile> {
    let templates = [
        ("genmitsu-3018", "Genmitsu 3018-Pro", GENMITSU_3018_TEMPLATE),
        ("masso-g3-no-atc", "Masso G3 - Manual Tool Change", MASSO_G3_NO_ATC_TEMPLATE),
        ("masso-g3-with-atc", "Masso G3 - With ATC", MASSO_G3_WITH_ATC_TEMPLATE),
    ];

    templates
        .iter()
        .map(|(key, fallback_name, raw)| parse_machine_template_yaml(raw, key, fallback_name))
        .collect()
}

fn parse_machine_template_yaml(text: &str, key: &str, fallback_name: &str) -> LibraryProfile {
    let mut machine = MachineProfile::default();
    machine.id = format!("template-{}", slug(key));
    machine.name = fallback_name.to_string();
    machine.built_in = true;

    let mut pending_required_fields = BTreeSet::new();
    let required_paths = cnc_schema_required_paths();

    let mark_missing = |pending: &mut BTreeSet<String>, path: &str| {
        if required_paths.contains(path) {
            pending.insert(path.to_string());
        }
    };

    let root: serde_yaml::Value = serde_yaml::from_str(text).unwrap_or(serde_yaml::Value::Null);
    if let Some(name) = root.get("name").and_then(|v| v.as_str()) {
        if !name.trim().is_empty() {
            machine.name = name.trim().to_string();
        }
    }

    let machine_node = root.get("machine");

    let fixture = machine_node.and_then(|m| m.get("fixture_plate"));
    let fx = fixture.and_then(|f| f.get("x")).and_then(|v| v.as_str());
    if let Some(raw) = fx {
        if let Ok(length) = Length::from_string(raw, Some(LengthUnit::Mm)) {
            machine.fixture_plate_max_x = length.as_mm().round().max(0.0) as u32;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.fixture_plate.x");
    }

    let fy = fixture.and_then(|f| f.get("y")).and_then(|v| v.as_str());
    if let Some(raw) = fy {
        if let Ok(length) = Length::from_string(raw, Some(LengthUnit::Mm)) {
            machine.fixture_plate_max_y = length.as_mm().round().max(0.0) as u32;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.fixture_plate.y");
    }

    let max_feed = machine_node
        .and_then(|m| m.get("max_feed_rate"))
        .and_then(|v| v.as_str());
    if let Some(raw) = max_feed {
        if let Ok(rate) = FeedRate::from_string(raw, Some(FeedRateUnit::MmPerMin)) {
            machine.max_feed_rate_mm_per_min = rate.as_mm_per_min().round().max(0.0) as u32;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.max_feed_rate");
    }

    let spindle_min = machine_node
        .and_then(|m| m.get("spindle_rpm_min"))
        .and_then(|v| v.as_str());
    if let Some(raw) = spindle_min {
        if let Ok(value) = RotationalSpeed::from_string(raw, Some(RotationalSpeedUnit::Rpm)) {
            machine.spindle_min_rpm = value.as_rpm().round().max(0.0) as u32;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.spindle_rpm_min");
    }

    let spindle_max = machine_node
        .and_then(|m| m.get("spindle_rpm_max"))
        .and_then(|v| v.as_str());
    if let Some(raw) = spindle_max {
        if let Ok(value) = RotationalSpeed::from_string(raw, Some(RotationalSpeedUnit::Rpm)) {
            machine.spindle_max_rpm = value.as_rpm().round().max(0.0) as u32;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.spindle_rpm_max");
    }

    if let Some(atc) = machine_node
        .and_then(|m| m.get("atc_slot_count"))
        .and_then(|v| v.as_i64())
    {
        machine.atc_slot_count = atc.clamp(0, u8::MAX as i64) as u8;
    } else {
        mark_missing(&mut pending_required_fields, "machine.atc_slot_count");
    }

    let origin = machine_node.and_then(|m| m.get("origin"));
    if let Some(x0) = origin.and_then(|o| o.get("x0")).and_then(|v| v.as_str()) {
        machine.origin_x0 = capitalize_axis_origin(x0, "Left");
    } else {
        mark_missing(&mut pending_required_fields, "machine.origin.x0");
    }
    if let Some(y0) = origin.and_then(|o| o.get("y0")).and_then(|v| v.as_str()) {
        machine.origin_y0 = capitalize_axis_origin(y0, "Front");
    } else {
        mark_missing(&mut pending_required_fields, "machine.origin.y0");
    }

    let scaling = machine_node.and_then(|m| m.get("scaling"));
    if let Some(x) = scaling.and_then(|s| s.get("x")).and_then(|v| v.as_f64()) {
        machine.scaling_x = x as f32;
    } else {
        mark_missing(&mut pending_required_fields, "machine.scaling.x");
    }
    if let Some(y) = scaling.and_then(|s| s.get("y")).and_then(|v| v.as_f64()) {
        machine.scaling_y = y as f32;
    } else {
        mark_missing(&mut pending_required_fields, "machine.scaling.y");
    }

    if let Some(increment) = machine_node
        .and_then(|m| m.get("line_numbering_increment"))
        .and_then(|v| v.as_i64())
    {
        machine.line_numbering_increment = increment.max(0) as u16;
    } else {
        mark_missing(&mut pending_required_fields, "machine.line_numbering_increment");
    }

    machine.pending_required_fields = pending_required_fields;
    machine.usable = machine.pending_required_fields.is_empty();

    LibraryProfile {
        key: key.to_string(),
        name: machine.name.clone(),
        machine,
    }
}

fn capitalize_axis_origin(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let mut chars = trimmed.chars();
    let first = chars
        .next()
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    let rest = chars.as_str().to_ascii_lowercase();
    format!("{}{}", first, rest)
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
