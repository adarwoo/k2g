use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use serde::Serialize;
use std::fs;

use super::super::model::*;
use crate::ui::unit_service;
use crate::units::{FeedRate, Length};

#[component]
pub fn CncScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);

    let Some(machine) = snapshot.selected_machine().cloned() else {
        return rsx! {
            div { class: "screen single centered",
                p { "No CNC profile selected. Add or select one in CNC profiles." }
            }
        };
    };

    let selected_id = machine.id.clone();
    let delete_selected_id = selected_id.clone();

    let mut field_error_message = use_signal(|| None::<String>);
    let mut fixture_x_is_editing = use_signal(|| false);
    let mut fixture_x_draft = use_signal(String::new);
    let mut fixture_y_is_editing = use_signal(|| false);
    let mut fixture_y_draft = use_signal(String::new);
    let mut feed_is_editing = use_signal(|| false);
    let mut feed_draft = use_signal(String::new);
    let mut spindle_min_is_editing = use_signal(|| false);
    let mut spindle_min_draft = use_signal(String::new);
    let mut spindle_max_is_editing = use_signal(|| false);
    let mut spindle_max_draft = use_signal(String::new);
    let mut scaling_error_message = use_signal(|| None::<String>);
    let mut scaling_x_is_editing = use_signal(|| false);
    let mut scaling_x_draft = use_signal(String::new);
    let mut scaling_y_is_editing = use_signal(|| false);
    let mut scaling_y_draft = use_signal(String::new);

    let fixture_x_value = Length::from_mm(machine.fixture_plate_max_x as f64);
    let fixture_y_value = Length::from_mm(machine.fixture_plate_max_y as f64);
    let feed_value = FeedRate::from_mm_per_min(machine.max_feed_rate_mm_per_min as f64);
    let spindle_min_value = crate::units::RotationalSpeed::from_rpm(machine.spindle_min_rpm as f64);
    let spindle_max_value = crate::units::RotationalSpeed::from_rpm(machine.spindle_max_rpm as f64);

    let fixture_x_edit_seed =
        unit_service::format_length_edit_display(fixture_x_value, snapshot.unit_system);
    let fixture_y_edit_seed =
        unit_service::format_length_edit_display(fixture_y_value, snapshot.unit_system);
    let feed_edit_seed = unit_service::format_feed_edit_display(feed_value, snapshot.unit_system);
    let spindle_min_edit_seed = unit_service::format_rotational_speed_edit_display(spindle_min_value);
    let spindle_max_edit_seed = unit_service::format_rotational_speed_edit_display(spindle_max_value);
    let scaling_x_edit_seed = unit_service::format_percentage_edit_display(machine.scaling_x as f64);
    let scaling_y_edit_seed = unit_service::format_percentage_edit_display(machine.scaling_y as f64);

    let fixture_x_display =
        unit_service::format_length_display(fixture_x_value, snapshot.unit_system);
    let fixture_y_display =
        unit_service::format_length_display(fixture_y_value, snapshot.unit_system);
    let feed_display = unit_service::format_feed_display(feed_value, snapshot.unit_system);
    let spindle_min_display = unit_service::format_rotational_speed_display(spindle_min_value);
    let spindle_max_display = unit_service::format_rotational_speed_display(spindle_max_value);
    let scaling_x_display = unit_service::format_percentage_display(machine.scaling_x as f64);
    let scaling_y_display = unit_service::format_percentage_display(machine.scaling_y as f64);
    let default_machine = MachineProfile::default();
    let header_rows = rows_for_template(&default_machine.gcode_header, 6, 18);
    let footer_rows = rows_for_template(&default_machine.gcode_footer, 2, 8);
    let route_plunge_rows = rows_for_template(&default_machine.route_plunge_and_offset, 3, 12);
    let manual_prompt_rows = rows_for_template(&default_machine.tool_change_manual_prompt, 2, 8);
    let tool_change_rows = rows_for_template(&default_machine.tool_change_command, 4, 12);

    rsx! {
        div { class: "screen split",
            section { class: "panel grow",
                div { class: "panel-header",
                    h3 { "CNC profile: {machine.name}" }
                    div { class: "actions",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| {
                                state.with_mut(|s| s.clone_selected_machine());
                                status_message
                                    .set("CNC profile duplicated. Update the name as needed.".to_string());
                            },
                            "Duplicate"
                        }
                        button {
                            class: "btn btn-danger",
                            onclick: move |_| {
                                let cnc_id = delete_selected_id.clone();
                                let impact = state.read().impact_delete_cnc_profile(&cnc_id);
                                let description = format_impact_warning(
                                    "Delete CNC profile and dependent assets?",
                                    &impact,
                                );
                                let confirmed = MessageDialog::new()
                                    .set_level(MessageLevel::Warning)
                                    .set_title("Delete CNC profile")
                                    .set_description(&description)
                                    .set_buttons(MessageButtons::YesNo)
                                    .show();
                                if confirmed == rfd::MessageDialogResult::Yes {
                                    let impact = state.with_mut(|s| s.delete_cnc_profile_with_cascade(&cnc_id));
                                    status_message.set(format_impact_summary("Deleted CNC profile", &impact));
                                }
                            },
                            "Delete profile"
                        }
                        button {
                            class: "btn btn-secondary",
                            onclick: {
                                let machine = machine.clone();
                                move |_| {
                                    let default_name = format!(
                                        "{}.cnc-profile.yaml",
                                        slug_file_name(&machine.name),
                                    );
                                    let picked = FileDialog::new()
                                        .set_title("Export CNC profile")
                                        .set_file_name(&default_name)
                                        .add_filter("CNC profile YAML", &["yaml", "yml"])
                                        .save_file();
                                    let Some(path) = picked else {
                                        return;
                                    };
                                    let mut output_path = path;
                                    let file_name = output_path
                                        .file_name()
                                        .and_then(|f| f.to_str())
                                        .unwrap_or_default()
                                        .to_ascii_lowercase();
                                    if !file_name.ends_with(".cnc-profile.yaml")
                                        && !file_name.ends_with(".cnc-profile.yml")
                                    {
                                        let stem = output_path
                                            .file_stem()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("cnc-profile");
                                        let new_name = format!("{}.cnc-profile.yaml", stem);
                                        output_path = output_path.with_file_name(new_name);
                                    }
                                    let yaml = match machine_profile_to_yaml(&machine) {
                                        Ok(v) => v,
                                        Err(_) => {
                                            status_message
                                                .set("Export failed: unable to serialize profile".to_string());
                                            return;
                                        }
                                    };
                                    if fs::write(&output_path, yaml).is_ok() {
                                        status_message.set("CNC profile exported".to_string());
                                    } else {
                                        status_message.set("Export failed: unable to write file".to_string());
                                    }
                                }
                            },
                            "Export profile"
                        }
                    }
                }

                p { "This page configures the currently selected CNC profile." }

                div { class: "edit-grid",
                    div { class: "field",
                        label { "Profile name" }
                        input {
                            value: machine.name.clone(),
                            autofocus: snapshot.focus_profile_name_editor,
                            oninput: move |evt| {
                                let proposed = evt.value();
                                state
                                    .with_mut(|s| {
                                        match s.rename_selected_machine(&proposed) {
                                            Ok(_) => {
                                                s.focus_profile_name_editor = false;
                                                status_message.set(String::new());
                                            }
                                            Err(msg) => {
                                                status_message.set(msg);
                                            }
                                        }
                                    });
                            },
                        }
                    }

                    div { class: "field",
                        label { "ATC slots" }
                        input {
                            r#type: "number",
                            min: "0",
                            value: "{machine.atc_slot_count}",
                            oninput: {
                                let selected_id = selected_id.clone();
                                move |evt| {
                                    let value = evt.value().parse::<u8>().unwrap_or(0);
                                    state
                                        .with_mut(|s| {
                                            if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                            {
                                                t.atc_slot_count = value;
                                            }
                                            s.seed_rack_slots(value);
                                        });
                                }
                            },
                        }
                    }

                    div { class: "field section-block",
                        h4 { "Fixture plate" }

                        if let Some(message) = field_error_message.read().clone() {
                            p { class: "diag-status", "{message}" }
                        }

                        div { class: "field section-subfield",
                            label { "Fixture X" }
                            div { class: "sub-field",
                                if *fixture_x_is_editing.read() {
                                    input {
                                        class: "stock-detail-input",
                                        value: fixture_x_draft.read().clone(),
                                        autofocus: true,
                                        onmounted: move |evt| async move {
                                            let _ = evt.set_focus(true).await;
                                        },
                                        oninput: move |evt| {
                                            fixture_x_draft.set(evt.value());
                                        },
                                        onkeydown: {
                                            let fixture_x_edit_seed = fixture_x_edit_seed.clone();
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let raw = fixture_x_draft.read().trim().to_string();
                                                    match unit_service::parse_length_with_preference(
                                                        &raw,
                                                        snapshot.unit_system,
                                                    ) {
                                                        Ok(length) if length.as_mm() >= 0.0 => {
                                                            state
                                                                .with_mut(|s| {
                                                                    if let Some(t) = s
                                                                        .machines
                                                                        .iter_mut()
                                                                        .find(|m| m.id == selected_id)
                                                                    {
                                                                        t.fixture_plate_max_x = length.as_mm().round().max(0.0)
                                                                            as u32;
                                                                    }
                                                                });
                                                            fixture_x_is_editing.set(false);
                                                            field_error_message.set(None);
                                                        }
                                                        _ => {
                                                            field_error_message
                                                                .set(
                                                                    Some(
                                                                        "Fixture X must be a valid non-negative length".to_string(),
                                                                    ),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    fixture_x_draft.set(fixture_x_edit_seed.clone());
                                                    fixture_x_is_editing.set(false);
                                                    field_error_message.set(None);
                                                }
                                            }
                                        },
                                        onfocusout: {
                                            let fixture_x_edit_seed = fixture_x_edit_seed.clone();
                                            move |_| {
                                                fixture_x_draft.set(fixture_x_edit_seed.clone());
                                                fixture_x_is_editing.set(false);
                                            }
                                        },
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "stock-detail-input stock-detail-trigger",
                                        onclick: {
                                            let fixture_x_edit_seed = fixture_x_edit_seed.clone();
                                            move |_| {
                                                fixture_x_is_editing.set(true);
                                                fixture_x_draft.set(fixture_x_edit_seed.clone());
                                                field_error_message.set(None);
                                            }
                                        },
                                        "{fixture_x_display}"
                                    }
                                }
                            }
                        }

                        div { class: "field section-subfield",
                            label { "Fixture Y" }
                            div { class: "sub-field",
                                if *fixture_y_is_editing.read() {
                                    input {
                                        class: "stock-detail-input",
                                        value: fixture_y_draft.read().clone(),
                                        autofocus: true,
                                        onmounted: move |evt| async move {
                                            let _ = evt.set_focus(true).await;
                                        },
                                        oninput: move |evt| {
                                            fixture_y_draft.set(evt.value());
                                        },
                                        onkeydown: {
                                            let fixture_y_edit_seed = fixture_y_edit_seed.clone();
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let raw = fixture_y_draft.read().trim().to_string();
                                                    match unit_service::parse_length_with_preference(
                                                        &raw,
                                                        snapshot.unit_system,
                                                    ) {
                                                        Ok(length) if length.as_mm() >= 0.0 => {
                                                            state
                                                                .with_mut(|s| {
                                                                    if let Some(t) = s
                                                                        .machines
                                                                        .iter_mut()
                                                                        .find(|m| m.id == selected_id)
                                                                    {
                                                                        t.fixture_plate_max_y = length.as_mm().round().max(0.0)
                                                                            as u32;
                                                                    }
                                                                });
                                                            fixture_y_is_editing.set(false);
                                                            field_error_message.set(None);
                                                        }
                                                        _ => {
                                                            field_error_message
                                                                .set(
                                                                    Some(
                                                                        "Fixture Y must be a valid non-negative length".to_string(),
                                                                    ),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    fixture_y_draft.set(fixture_y_edit_seed.clone());
                                                    fixture_y_is_editing.set(false);
                                                    field_error_message.set(None);
                                                }
                                            }
                                        },
                                        onfocusout: {
                                            let fixture_y_edit_seed = fixture_y_edit_seed.clone();
                                            move |_| {
                                                fixture_y_draft.set(fixture_y_edit_seed.clone());
                                                fixture_y_is_editing.set(false);
                                            }
                                        },
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "stock-detail-input stock-detail-trigger",
                                        onclick: {
                                            let fixture_y_edit_seed = fixture_y_edit_seed.clone();
                                            move |_| {
                                                fixture_y_is_editing.set(true);
                                                fixture_y_draft.set(fixture_y_edit_seed.clone());
                                                field_error_message.set(None);
                                            }
                                        },
                                        "{fixture_y_display}"
                                    }
                                }
                            }
                        }

                        div { class: "field section-subfield",
                            label { "Max feed rate" }
                            div { class: "sub-field",
                                if *feed_is_editing.read() {
                                    input {
                                        class: "stock-detail-input",
                                        value: feed_draft.read().clone(),
                                        autofocus: true,
                                        onmounted: move |evt| async move {
                                            let _ = evt.set_focus(true).await;
                                        },
                                        oninput: move |evt| {
                                            feed_draft.set(evt.value());
                                        },
                                        onkeydown: {
                                            let feed_edit_seed = feed_edit_seed.clone();
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let raw = feed_draft.read().trim().to_string();
                                                    match unit_service::parse_feed_with_preference(
                                                        &raw,
                                                        snapshot.unit_system,
                                                    ) {
                                                        Ok(feed_rate) if feed_rate.as_mm_per_min() >= 0.0 => {
                                                            state
                                                                .with_mut(|s| {
                                                                    if let Some(t) = s
                                                                        .machines
                                                                        .iter_mut()
                                                                        .find(|m| m.id == selected_id)
                                                                    {
                                                                        t.max_feed_rate_mm_per_min = feed_rate
                                                                            .as_mm_per_min()
                                                                            .round()
                                                                            .max(0.0) as u32;
                                                                    }
                                                                });
                                                            feed_is_editing.set(false);
                                                            field_error_message.set(None);
                                                        }
                                                        _ => {
                                                            field_error_message
                                                                .set(
                                                                    Some(
                                                                        "Max feed rate must be a valid non-negative feed rate"
                                                                            .to_string(),
                                                                    ),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    feed_draft.set(feed_edit_seed.clone());
                                                    feed_is_editing.set(false);
                                                    field_error_message.set(None);
                                                }
                                            }
                                        },
                                        onfocusout: {
                                            let feed_edit_seed = feed_edit_seed.clone();
                                            move |_| {
                                                feed_draft.set(feed_edit_seed.clone());
                                                feed_is_editing.set(false);
                                            }
                                        },
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "stock-detail-input stock-detail-trigger",
                                        onclick: {
                                            let feed_edit_seed = feed_edit_seed.clone();
                                            move |_| {
                                                feed_is_editing.set(true);
                                                feed_draft.set(feed_edit_seed.clone());
                                                field_error_message.set(None);
                                            }
                                        },
                                        "{feed_display}"
                                    }
                                }
                            }
                        }
                    }

                    div { class: "field section-block",
                        h4 { "Spindle" }

                        div { class: "field section-subfield",
                            label { "Min" }
                            div { class: "sub-field",
                                if *spindle_min_is_editing.read() {
                                    input {
                                        class: "stock-detail-input",
                                        value: spindle_min_draft.read().clone(),
                                        autofocus: true,
                                        onmounted: move |evt| async move {
                                            let _ = evt.set_focus(true).await;
                                        },
                                        oninput: move |evt| {
                                            spindle_min_draft.set(evt.value());
                                        },
                                        onkeydown: {
                                            let spindle_min_edit_seed = spindle_min_edit_seed.clone();
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let raw = spindle_min_draft.read().trim().to_string();
                                                    match unit_service::parse_rotational_speed(&raw) {
                                                        Ok(speed) if speed.as_rpm() >= 0.0 => {
                                                            state
                                                                .with_mut(|s| {
                                                                    if let Some(t) = s
                                                                        .machines
                                                                        .iter_mut()
                                                                        .find(|m| m.id == selected_id)
                                                                    {
                                                                        t.spindle_min_rpm = speed.as_rpm().round().max(0.0) as u32;
                                                                    }
                                                                });
                                                            spindle_min_is_editing.set(false);
                                                            field_error_message.set(None);
                                                        }
                                                        _ => {
                                                            field_error_message
                                                                .set(
                                                                    Some(
                                                                        "Spindle min must be a valid non-negative rpm value"
                                                                            .to_string(),
                                                                    ),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    spindle_min_draft.set(spindle_min_edit_seed.clone());
                                                    spindle_min_is_editing.set(false);
                                                    field_error_message.set(None);
                                                }
                                            }
                                        },
                                        onfocusout: {
                                            let spindle_min_edit_seed = spindle_min_edit_seed.clone();
                                            move |_| {
                                                spindle_min_draft.set(spindle_min_edit_seed.clone());
                                                spindle_min_is_editing.set(false);
                                            }
                                        },
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "stock-detail-input stock-detail-trigger",
                                        onclick: {
                                            let spindle_min_edit_seed = spindle_min_edit_seed.clone();
                                            move |_| {
                                                spindle_min_is_editing.set(true);
                                                spindle_min_draft.set(spindle_min_edit_seed.clone());
                                                field_error_message.set(None);
                                            }
                                        },
                                        "{spindle_min_display}"
                                    }
                                }
                            }
                        }

                        div { class: "field section-subfield",
                            label { "Max" }
                            div { class: "sub-field",
                                if *spindle_max_is_editing.read() {
                                    input {
                                        class: "stock-detail-input",
                                        value: spindle_max_draft.read().clone(),
                                        autofocus: true,
                                        onmounted: move |evt| async move {
                                            let _ = evt.set_focus(true).await;
                                        },
                                        oninput: move |evt| {
                                            spindle_max_draft.set(evt.value());
                                        },
                                        onkeydown: {
                                            let spindle_max_edit_seed = spindle_max_edit_seed.clone();
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let key = evt.key().to_string().to_ascii_lowercase();
                                                if key == "enter" || key == "numpadenter" {
                                                    let raw = spindle_max_draft.read().trim().to_string();
                                                    match unit_service::parse_rotational_speed(&raw) {
                                                        Ok(speed) if speed.as_rpm() >= 0.0 => {
                                                            state
                                                                .with_mut(|s| {
                                                                    if let Some(t) = s
                                                                        .machines
                                                                        .iter_mut()
                                                                        .find(|m| m.id == selected_id)
                                                                    {
                                                                        t.spindle_max_rpm = speed.as_rpm().round().max(0.0) as u32;
                                                                    }
                                                                });
                                                            spindle_max_is_editing.set(false);
                                                            field_error_message.set(None);
                                                        }
                                                        _ => {
                                                            field_error_message
                                                                .set(
                                                                    Some(
                                                                        "Spindle max must be a valid non-negative rpm value"
                                                                            .to_string(),
                                                                    ),
                                                                );
                                                        }
                                                    }
                                                } else if key == "escape" || key == "esc" {
                                                    evt.stop_propagation();
                                                    spindle_max_draft.set(spindle_max_edit_seed.clone());
                                                    spindle_max_is_editing.set(false);
                                                    field_error_message.set(None);
                                                }
                                            }
                                        },
                                        onfocusout: {
                                            let spindle_max_edit_seed = spindle_max_edit_seed.clone();
                                            move |_| {
                                                spindle_max_draft.set(spindle_max_edit_seed.clone());
                                                spindle_max_is_editing.set(false);
                                            }
                                        },
                                    }
                                } else {
                                    button {
                                        r#type: "button",
                                        class: "stock-detail-input stock-detail-trigger",
                                        onclick: {
                                            let spindle_max_edit_seed = spindle_max_edit_seed.clone();
                                            move |_| {
                                                spindle_max_is_editing.set(true);
                                                spindle_max_draft.set(spindle_max_edit_seed.clone());
                                                field_error_message.set(None);
                                            }
                                        },
                                        "{spindle_max_display}"
                                    }
                                }
                            }
                        }
                    }

                    div { class: "field section-block",
                        h4 { "Coordinate origin" }

                        div { class: "field section-subfield",
                            label { "X axis origin" }
                            select {
                                value: "{machine.origin_x0}",
                                onchange: {
                                    let selected_id = selected_id.clone();
                                    move |evt| {
                                        let value = evt.value();
                                        state
                                            .with_mut(|s| {
                                                if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                {
                                                    t.origin_x0 = value;
                                                }
                                            });
                                    }
                                },
                                option { value: "Left", "Left" }
                                option { value: "Right", "Right" }
                                option { value: "Front", "Front" }
                                option { value: "Back", "Back" }
                            }
                        }

                        div { class: "field section-subfield",
                            label { "Y axis origin" }
                            select {
                                value: "{machine.origin_y0}",
                                onchange: {
                                    let selected_id = selected_id.clone();
                                    move |evt| {
                                        let value = evt.value();
                                        state
                                            .with_mut(|s| {
                                                if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                {
                                                    t.origin_y0 = value;
                                                }
                                            });
                                    }
                                },
                                option { value: "Front", "Front" }
                                option { value: "Back", "Back" }
                                option { value: "Left", "Left" }
                                option { value: "Right", "Right" }
                            }
                        }
                    }

                    div { class: "field section-block",
                        h4 { "Axis scaling" }

                        if let Some(message) = scaling_error_message.read().clone() {
                            p { class: "diag-status", "{message}" }
                        }

                        div { class: "field section-subfield",
                            label { "X scale" }
                            if *scaling_x_is_editing.read() {
                                input {
                                    class: "stock-detail-input",
                                    value: scaling_x_draft.read().clone(),
                                    autofocus: true,
                                    onmounted: move |evt| async move {
                                        let _ = evt.set_focus(true).await;
                                    },
                                    oninput: move |evt| {
                                        scaling_x_draft.set(evt.value());
                                    },
                                    onkeydown: {
                                        let scaling_x_edit_seed = scaling_x_edit_seed.clone();
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let key = evt.key().to_string().to_ascii_lowercase();
                                            if key == "enter" || key == "numpadenter" {
                                                let raw = scaling_x_draft.read().trim().to_string();
                                                match unit_service::parse_percentage(&raw) {
                                                    Ok(value) if (1.0..=500.0).contains(&value) => {
                                                        state
                                                            .with_mut(|s| {
                                                                if let Some(t) = s
                                                                    .machines
                                                                    .iter_mut()
                                                                    .find(|m| m.id == selected_id)
                                                                {
                                                                    t.scaling_x = value as f32;
                                                                }
                                                            });
                                                        scaling_x_is_editing.set(false);
                                                        scaling_error_message.set(None);
                                                    }
                                                    _ => {
                                                        scaling_error_message
                                                            .set(Some("X scale must be between 1 and 500".to_string()));
                                                    }
                                                }
                                            } else if key == "escape" || key == "esc" {
                                                evt.stop_propagation();
                                                scaling_x_draft.set(scaling_x_edit_seed.clone());
                                                scaling_x_is_editing.set(false);
                                                scaling_error_message.set(None);
                                            }
                                        }
                                    },
                                    onfocusout: {
                                        let scaling_x_edit_seed = scaling_x_edit_seed.clone();
                                        move |_| {
                                            scaling_x_draft.set(scaling_x_edit_seed.clone());
                                            scaling_x_is_editing.set(false);
                                        }
                                    },
                                }
                            } else {
                                button {
                                    r#type: "button",
                                    class: "stock-detail-input stock-detail-trigger",
                                    onclick: {
                                        let scaling_x_edit_seed = scaling_x_edit_seed.clone();
                                        move |_| {
                                            scaling_x_is_editing.set(true);
                                            scaling_x_draft.set(scaling_x_edit_seed.clone());
                                            scaling_error_message.set(None);
                                        }
                                    },
                                    "{scaling_x_display}"
                                }
                            }
                        }

                        div { class: "field section-subfield",
                            label { "Y scale" }
                            if *scaling_y_is_editing.read() {
                                input {
                                    class: "stock-detail-input",
                                    value: scaling_y_draft.read().clone(),
                                    autofocus: true,
                                    onmounted: move |evt| async move {
                                        let _ = evt.set_focus(true).await;
                                    },
                                    oninput: move |evt| {
                                        scaling_y_draft.set(evt.value());
                                    },
                                    onkeydown: {
                                        let scaling_y_edit_seed = scaling_y_edit_seed.clone();
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let key = evt.key().to_string().to_ascii_lowercase();
                                            if key == "enter" || key == "numpadenter" {
                                                let raw = scaling_y_draft.read().trim().to_string();
                                                match unit_service::parse_percentage(&raw) {
                                                    Ok(value) if (1.0..=500.0).contains(&value) => {
                                                        state
                                                            .with_mut(|s| {
                                                                if let Some(t) = s
                                                                    .machines
                                                                    .iter_mut()
                                                                    .find(|m| m.id == selected_id)
                                                                {
                                                                    t.scaling_y = value as f32;
                                                                }
                                                            });
                                                        scaling_y_is_editing.set(false);
                                                        scaling_error_message.set(None);
                                                    }
                                                    _ => {
                                                        scaling_error_message
                                                            .set(Some("Y scale must be between 1 and 500".to_string()));
                                                    }
                                                }
                                            } else if key == "escape" || key == "esc" {
                                                evt.stop_propagation();
                                                scaling_y_draft.set(scaling_y_edit_seed.clone());
                                                scaling_y_is_editing.set(false);
                                                scaling_error_message.set(None);
                                            }
                                        }
                                    },
                                    onfocusout: {
                                        let scaling_y_edit_seed = scaling_y_edit_seed.clone();
                                        move |_| {
                                            scaling_y_draft.set(scaling_y_edit_seed.clone());
                                            scaling_y_is_editing.set(false);
                                        }
                                    },
                                }
                            } else {
                                button {
                                    r#type: "button",
                                    class: "stock-detail-input stock-detail-trigger",
                                    onclick: {
                                        let scaling_y_edit_seed = scaling_y_edit_seed.clone();
                                        move |_| {
                                            scaling_y_is_editing.set(true);
                                            scaling_y_draft.set(scaling_y_edit_seed.clone());
                                            scaling_error_message.set(None);
                                        }
                                    },
                                    "{scaling_y_display}"
                                }
                            }
                        }
                    }

                    div { class: "field section-block",
                        h4 { "Line numbering" }

                        div { class: "field section-subfield",
                            label { class: "checkbox-line",
                                input {
                                    r#type: "checkbox",
                                    checked: machine.line_numbering_enabled,
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let enabled = evt.checked();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.line_numbering_enabled = enabled;
                                                    }
                                                });
                                        }
                                    },
                                }
                                span { "Emit line numbers" }
                            }
                        }

                        if machine.line_numbering_enabled {
                            div { class: "field section-subfield",
                                label { "Increment" }
                                input {
                                    r#type: "number",
                                    min: "1",
                                    step: "1",
                                    value: "{machine.line_numbering_increment}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value().parse::<u32>().unwrap_or(10).max(1);
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.line_numbering_increment = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }
                        }
                    }

                    div { class: "field section-block full-width",
                        h4 { "G-code templates" }

                        p { class: "diag-status",
                            "Use {{placeholders}} where documented. Unknown placeholders are preserved as-is."
                        }

                        div { class: "field section-subfield section-block",
                            h4 { "Header / Footer" }

                            div { class: "field section-subfield",
                                label { "Header" }
                                textarea {
                                    class: "gcode-editor cnc-template-editor",
                                    rows: "{header_rows}",
                                    value: "{machine.gcode_header}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.gcode_header = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }

                            div { class: "field section-subfield",
                                label { "Footer" }
                                textarea {
                                    class: "gcode-editor cnc-template-editor",
                                    rows: "{footer_rows}",
                                    value: "{machine.gcode_footer}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.gcode_footer = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }
                        }

                        div { class: "field section-subfield section-block",
                            h4 { "Drill cycle" }

                            for (lbl , getter , setter) in [
                                ("First move", machine.drill_first_move.clone(), "drill_first_move"),
                                (
                                    "Cycle mode (last)",
                                    machine.drill_cycle_mode_last.clone(),
                                    "drill_cycle_mode_last",
                                ),
                                (
                                    "Cycle mode (series)",
                                    machine.drill_cycle_mode_series.clone(),
                                    "drill_cycle_mode_series",
                                ),
                                ("Cycle start", machine.drill_cycle_start.clone(), "drill_cycle_start"),
                                ("Next hole", machine.drill_next_hole.clone(), "drill_next_hole"),
                                ("Cycle cancel", machine.drill_cycle_cancel.clone(), "drill_cycle_cancel"),
                            ]
                            {
                                div { class: "field section-subfield",
                                    label { "{lbl}" }
                                    input {
                                        value: "{getter}",
                                        oninput: {
                                            let selected_id = selected_id.clone();
                                            let field = setter.to_string();
                                            move |evt| {
                                                let value = evt.value();
                                                let field = field.clone();
                                                state
                                                    .with_mut(|s| {
                                                        if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                        {
                                                            match field.as_str() {
                                                                "drill_first_move" => t.drill_first_move = value,
                                                                "drill_cycle_mode_last" => t.drill_cycle_mode_last = value,
                                                                "drill_cycle_mode_series" => {
                                                                    t.drill_cycle_mode_series = value;
                                                                }
                                                                "drill_cycle_start" => t.drill_cycle_start = value,
                                                                "drill_next_hole" => t.drill_next_hole = value,
                                                                "drill_cycle_cancel" => t.drill_cycle_cancel = value,
                                                                _ => {}
                                                            }
                                                        }
                                                    });
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        div { class: "field section-subfield section-block",
                            h4 { "Routing" }

                            div { class: "field section-subfield",
                                label { "Plunge and offset" }
                                textarea {
                                    class: "gcode-editor cnc-template-editor",
                                    rows: "{route_plunge_rows}",
                                    value: "{machine.route_plunge_and_offset}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.route_plunge_and_offset = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }

                            for (lbl , getter , setter) in [
                                ("Arc UP", machine.route_arc_up.clone(), "route_arc_up"),
                                ("Arc DOWN", machine.route_arc_down.clone(), "route_arc_down"),
                                ("Retract", machine.route_retract.clone(), "route_retract"),
                            ]
                            {
                                div { class: "field section-subfield",
                                    label { "{lbl}" }
                                    input {
                                        value: "{getter}",
                                        oninput: {
                                            let selected_id = selected_id.clone();
                                            let field = setter.to_string();
                                            move |evt| {
                                                let value = evt.value();
                                                let field = field.clone();
                                                state
                                                    .with_mut(|s| {
                                                        if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                        {
                                                            match field.as_str() {
                                                                "route_arc_up" => t.route_arc_up = value,
                                                                "route_arc_down" => t.route_arc_down = value,
                                                                "route_retract" => t.route_retract = value,
                                                                _ => {}
                                                            }
                                                        }
                                                    });
                                            }
                                        },
                                    }
                                }
                            }
                        }

                        div { class: "field section-subfield section-block",
                            h4 { "Tool change" }

                            div { class: "field section-subfield",
                                label { "Manual prompt" }
                                p { class: "diag-status", "Only emitted when ATC is disabled." }
                                textarea {
                                    class: "gcode-editor cnc-template-editor",
                                    rows: "{manual_prompt_rows}",
                                    value: "{machine.tool_change_manual_prompt}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.tool_change_manual_prompt = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }

                            div { class: "field section-subfield",
                                label { "Command" }
                                textarea {
                                    class: "gcode-editor cnc-template-editor",
                                    rows: "{tool_change_rows}",
                                    value: "{machine.tool_change_command}",
                                    oninput: {
                                        let selected_id = selected_id.clone();
                                        move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    if let Some(t) = s.machines.iter_mut().find(|m| m.id == selected_id)
                                                    {
                                                        t.tool_change_command = value;
                                                    }
                                                });
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }

            section { class: "panel fixed",
                h3 { "CNC profile summary" }
                p { "ID: {machine.id}" }
                p { "Fixture size: {fixture_x_display} x {fixture_y_display}" }
                p { "Maximum feed rate: {feed_display}" }
                p { "Spindle: {spindle_min_display} – {spindle_max_display}" }
                p { "ATC slots: {machine.atc_slot_count}" }
                p { "Origin: {machine.origin_x0} / {machine.origin_y0}" }
                p { "Scaling: {machine.scaling_x}% × {machine.scaling_y}%" }
                if machine.line_numbering_enabled {
                    p { "Line numbering: every {machine.line_numbering_increment}" }
                }
                if !status_message.read().is_empty() {
                    p { class: "diag-status", "{status_message}" }
                }
            }
        }
    }
}

fn format_impact_warning(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    let mut lines = vec![prefix.to_string()];
    for item in impact.primary_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.dependent_job_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.deleted_live_jobs.iter() {
        lines.push(format!("- {}", item));
    }
    lines.join("\n")
}

fn format_impact_summary(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    format!(
        "{}: {} primary, {} dependent job profile(s), {} live job(s)",
        prefix,
        impact.primary_profiles.len(),
        impact.dependent_job_profiles.len(),
        impact.deleted_live_jobs.len()
    )
}

fn slug_file_name(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == ' ' || ch == '-' || ch == '_') && !out.ends_with('-') {
            out.push('-');
        }
    }

    if out.is_empty() { "cnc-profile".to_string() } else { out }
}

#[derive(Serialize)]
struct ExportFixture {
    x: String,
    y: String,
}

#[derive(Serialize)]
struct ExportOrigin {
    x0: String,
    y0: String,
}

#[derive(Serialize)]
struct ExportScaling {
    x: f32,
    y: f32,
}

#[derive(Serialize)]
struct ExportLineNumbering {
    enabled: bool,
    increment: u32,
}

#[derive(Serialize)]
struct ExportMachine {
    fixture_plate: ExportFixture,
    max_feed_rate: String,
    spindle_rpm_min: String,
    spindle_rpm_max: String,
    atc_slot_count: u8,
    origin: ExportOrigin,
    scaling: ExportScaling,
    line_numbering: ExportLineNumbering,
}

#[derive(Serialize)]
struct ExportDrill {
    first_move: String,
    cycle_mode_last: String,
    cycle_mode_series: String,
    cycle_start: String,
    next_hole: String,
    cycle_cancel: String,
}

#[derive(Serialize)]
struct ExportRoute {
    plunge_and_offset: String,
    arc_up: String,
    arc_down: String,
    retract: String,
}

#[derive(Serialize)]
struct ExportToolChange {
    manual_prompt: String,
    command: String,
}

#[derive(Serialize)]
struct ExportProfile {
    name: String,
    machine: ExportMachine,
    header: String,
    footer: String,
    drill: ExportDrill,
    route: ExportRoute,
    tool_change: ExportToolChange,
}

fn machine_profile_to_yaml(machine: &MachineProfile) -> Result<String, serde_yaml::Error> {
    let profile = ExportProfile {
        name: machine.name.clone(),
        machine: ExportMachine {
            fixture_plate: ExportFixture {
                x: format!("{}mm", machine.fixture_plate_max_x),
                y: format!("{}mm", machine.fixture_plate_max_y),
            },
            max_feed_rate: format!("{}mm/min", machine.max_feed_rate_mm_per_min),
            spindle_rpm_min: unit_service::format_rotational_speed_display(
                crate::units::RotationalSpeed::from_rpm(machine.spindle_min_rpm as f64),
            ),
            spindle_rpm_max: unit_service::format_rotational_speed_display(
                crate::units::RotationalSpeed::from_rpm(machine.spindle_max_rpm as f64),
            ),
            atc_slot_count: machine.atc_slot_count,
            origin: ExportOrigin {
                x0: machine.origin_x0.clone(),
                y0: machine.origin_y0.clone(),
            },
            scaling: ExportScaling {
                x: machine.scaling_x,
                y: machine.scaling_y,
            },
            line_numbering: ExportLineNumbering {
                enabled: machine.line_numbering_enabled,
                increment: machine.line_numbering_increment,
            },
        },
        header: machine.gcode_header.clone(),
        footer: machine.gcode_footer.clone(),
        drill: ExportDrill {
            first_move: machine.drill_first_move.clone(),
            cycle_mode_last: machine.drill_cycle_mode_last.clone(),
            cycle_mode_series: machine.drill_cycle_mode_series.clone(),
            cycle_start: machine.drill_cycle_start.clone(),
            next_hole: machine.drill_next_hole.clone(),
            cycle_cancel: machine.drill_cycle_cancel.clone(),
        },
        route: ExportRoute {
            plunge_and_offset: machine.route_plunge_and_offset.clone(),
            arc_up: machine.route_arc_up.clone(),
            arc_down: machine.route_arc_down.clone(),
            retract: machine.route_retract.clone(),
        },
        tool_change: ExportToolChange {
            manual_prompt: machine.tool_change_manual_prompt.clone(),
            command: machine.tool_change_command.clone(),
        },
    };

    serde_yaml::to_string(&profile)
}

fn rows_for_template(text: &str, min_rows: usize, max_rows: usize) -> usize {
    let lines = text.lines().count().max(1);
    lines.clamp(min_rows, max_rows)
}
