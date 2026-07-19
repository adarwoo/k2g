use dioxus::prelude::*;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use units::{FeedRate, FeedRateUnit, RotationalSpeed, RotationalSpeedUnit};
use super::super::model::*;
use super::setup_sections::{CatalogManagementPanel, GeneralSettingsPanel, MachineProfilesPanel, SetupSidebar, SetupTab};

const CNC_SCHEMA_TEXT: &str = include_str!("../../../resources/schemas/cnc.yaml");

pub(super) fn cnc_schema_required_paths() -> &'static BTreeSet<String> {
    static REQUIRED: OnceLock<BTreeSet<String>> = OnceLock::new();
    REQUIRED.get_or_init(|| parse_cnc_required_paths(CNC_SCHEMA_TEXT))
}

pub(super) fn cnc_required_field_label(key: &str) -> Option<&'static str> {
    match key {
        "machine.max_feed_rate" => Some("Max feed rate"),
        "machine.spindle_rpm_min" => Some("Spindle min"),
        "machine.spindle_rpm_max" => Some("Spindle max"),
        "machine.atc_slot_count" => Some("ATC slots"),
        "machine.scaling.x" => Some("X scale"),
        "machine.scaling.y" => Some("Y scale"),
        "machine.line_numbering_increment" => Some("Line numbering increment"),
        "primitives.initialise" => Some("Initialise primitive"),
        "primitives.rapid_move" => Some("Rapid move primitive"),
        "primitives.linear_cut" => Some("Linear cut primitive"),
        "primitives.start_spindle" => Some("Start spindle primitive"),
        "primitives.stop_spindle" => Some("Stop spindle primitive"),
        "primitives.drill" => Some("Drill primitive"),
        "primitives.peck_drill" => Some("Peck drill primitive"),
        "primitives.cut_arc" => Some("Cut arc primitive"),
        "primitives.cut_bezier" => Some("Cut bezier primitive"),
        "primitives.change_tool" => Some("Change tool primitive"),
        "primitives.conclude" => Some("Conclude primitive"),
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
pub fn SetupScreen(state: Signal<crate::app_state_impl::AppCtx>, boot: UiLaunchData) -> Element {
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

    let max_feed_rate = machine
        .get("max_feed_rate")
        .and_then(|v| v.as_str())
        .and_then(|s| FeedRate::from_string(s, Some(FeedRateUnit::MmPerMin)).ok())
        .unwrap_or_else(|| FeedRate::from_mm_per_min(2000.0));

    let spindle_rpm_min = RotationalSpeed::from_string(
        machine.get("spindle_rpm_min")?.as_str()?,
        Some(RotationalSpeedUnit::Rpm),
    )
    .ok()?;
    let spindle_rpm_max = RotationalSpeed::from_string(
        machine.get("spindle_rpm_max")?.as_str()?,
        Some(RotationalSpeedUnit::Rpm),
    )
    .ok()?;
    let atc_slot_count = machine.get("atc_slot_count")?.as_i64()? as u8;

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

    let primitives = root.get("primitives");
    let primitive_field = |key: &str, fallback: &str| {
        primitives
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(trim_nl)
            .unwrap_or_else(|| fallback.to_string())
    };

    let gcode_header = primitive_field("initialise", &def.gcode_header);
    let gcode_footer = primitive_field("conclude", &def.gcode_footer);
    let drill_first_move = primitive_field("rapid_move", &def.drill_first_move);
    let drill_cycle_mode_last = primitive_field("peck_drill", &def.drill_cycle_mode_last);
    let drill_cycle_mode_series = primitive_field("linear_cut", &def.drill_cycle_mode_series);
    let drill_cycle_start = primitive_field("start_spindle", &def.drill_cycle_start);
    let drill_next_hole = primitive_field("drill", &def.drill_next_hole);
    let drill_cycle_cancel = primitive_field("stop_spindle", &def.drill_cycle_cancel);
    let route_plunge_and_offset = primitive_field("cut_arc", &def.route_plunge_and_offset);
    let route_arc_up = primitive_field("cut_bezier", &def.route_arc_up);
    let route_arc_down = primitive_field("pause", &def.route_arc_down);
    let route_retract = primitive_field("banner", &def.route_retract);
    let tool_change_command = primitive_field("change_tool", &def.tool_change_command);

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
        max_feed_rate,
        spindle_rpm_min,
        spindle_rpm_max,
        atc_slot_count,
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

    let max_feed = machine_node
        .and_then(|m| m.get("max_feed_rate"))
        .and_then(|v| v.as_str());
    if let Some(raw) = max_feed {
        if let Ok(rate) = FeedRate::from_string(raw, Some(FeedRateUnit::MmPerMin)) {
            machine.max_feed_rate = rate;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.max_feed_rate");
    }

    let spindle_min = machine_node
        .and_then(|m| m.get("spindle_rpm_min"))
        .and_then(|v| v.as_str());
    if let Some(raw) = spindle_min {
        if let Ok(value) = RotationalSpeed::from_string(raw, Some(RotationalSpeedUnit::Rpm)) {
            machine.spindle_rpm_min = value;
        }
    } else {
        mark_missing(&mut pending_required_fields, "machine.spindle_rpm_min");
    }

    let spindle_max = machine_node
        .and_then(|m| m.get("spindle_rpm_max"))
        .and_then(|v| v.as_str());
    if let Some(raw) = spindle_max {
        if let Ok(value) = RotationalSpeed::from_string(raw, Some(RotationalSpeedUnit::Rpm)) {
            machine.spindle_rpm_max = value;
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

    let trim_nl = |s: &str| s.trim_end_matches('\n').to_string();
    let primitives = root.get("primitives");
    let primitive_field = |key: &str, fallback: &str| {
        primitives
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(trim_nl)
            .unwrap_or_else(|| fallback.to_string())
    };

    machine.gcode_header = primitive_field("initialise", &machine.gcode_header);
    machine.gcode_footer = primitive_field("conclude", &machine.gcode_footer);
    machine.drill_first_move = primitive_field("rapid_move", &machine.drill_first_move);
    machine.drill_cycle_mode_last = primitive_field("peck_drill", &machine.drill_cycle_mode_last);
    machine.drill_cycle_mode_series = primitive_field("linear_cut", &machine.drill_cycle_mode_series);
    machine.drill_cycle_start = primitive_field("start_spindle", &machine.drill_cycle_start);
    machine.drill_next_hole = primitive_field("drill", &machine.drill_next_hole);
    machine.drill_cycle_cancel = primitive_field("stop_spindle", &machine.drill_cycle_cancel);
    machine.route_plunge_and_offset = primitive_field("cut_arc", &machine.route_plunge_and_offset);
    machine.route_arc_up = primitive_field("cut_bezier", &machine.route_arc_up);
    machine.route_arc_down = primitive_field("pause", &machine.route_arc_down);
    machine.route_retract = primitive_field("banner", &machine.route_retract);
    machine.tool_change_command = primitive_field("change_tool", &machine.tool_change_command);

    for required_primitive in [
        "initialise",
        "rapid_move",
        "linear_cut",
        "start_spindle",
        "stop_spindle",
        "drill",
        "peck_drill",
        "cut_arc",
        "cut_bezier",
        "change_tool",
        "conclude",
    ] {
        if primitives.and_then(|p| p.get(required_primitive)).is_none() {
            mark_missing(
                &mut pending_required_fields,
                &format!("primitives.{required_primitive}"),
            );
        }
    }

    machine.pending_required_fields = pending_required_fields;
    machine.usable = machine.pending_required_fields.is_empty();

    LibraryProfile {
        key: key.to_string(),
        name: machine.name.clone(),
        machine,
    }
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
