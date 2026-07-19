use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use serde::Serialize;
use std::fs;

use super::super::model::*;
use super::profiles_common::{
    format_impact_warning, slug_file_name, ProfileLifecycleToolbar,
    ProfileNameDialog,
};
use super::setup::{
    cnc_profile_library, cnc_required_field_label, parse_machine_profile_yaml,
};
use crate::ui::unit_service;

#[component]
pub fn CncScreen(state: Signal<crate::ctx::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My CNC profile".to_string());
    let library_profiles = cnc_profile_library();
    let default_template_key = library_profiles
        .first()
        .map(|profile| profile.key.clone())
        .unwrap_or_default();
    let mut selected_template = use_signal(|| default_template_key.clone());

    let selected_machine = snapshot.selected_machine().cloned();
    let has_selected_machine = selected_machine.is_some();
    let selected_profile = selected_machine.clone();
    let has_selected_profile = selected_profile.is_some();
    let machine = selected_profile.clone().unwrap_or_else(|| MachineProfile {
        name: "None selected".to_string(),
        ..MachineProfile::default()
    });

    let profile_rows: Vec<MachineProfile> = snapshot
        .machines
        .iter()
        .filter(|machine| !machine.built_in)
        .cloned()
        .collect();

    let machine_options = profile_rows
        .iter()
        .map(|profile| {
            let suffix = if profile.usable { "" } else { " (not usable)" };
            (profile.id.clone(), format!("{}{}", profile.name, suffix))
        })
        .collect::<Vec<_>>();

    let selected_id = machine.id.clone();
    let is_built_in = machine.built_in;
    let editor_read_only = !has_selected_machine || is_built_in;
    let unit_system = snapshot.unit_system;

    let mut field_error_message = use_signal(|| None::<String>);
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

    let feed_value = machine.max_feed_rate;
    let spindle_min_value = machine.spindle_rpm_min;
    let spindle_max_value = machine.spindle_rpm_max;

    let feed_edit_seed = unit_service::format_feed_edit_display(feed_value, snapshot.unit_system);
    let spindle_min_edit_seed = unit_service::format_rotational_speed_edit_display(spindle_min_value);
    let spindle_max_edit_seed = unit_service::format_rotational_speed_edit_display(spindle_max_value);
    let scaling_x_edit_seed = unit_service::format_percentage_edit_display(machine.scaling_x as f64);
    let scaling_y_edit_seed = unit_service::format_percentage_edit_display(machine.scaling_y as f64);

    let feed_display = unit_service::format_feed_display(feed_value, snapshot.unit_system);
    let spindle_min_display = unit_service::format_rotational_speed_display(spindle_min_value);
    let spindle_max_display = unit_service::format_rotational_speed_display(spindle_max_value);
    let scaling_x_display = unit_service::format_percentage_display(machine.scaling_x as f64);
    let scaling_y_display = unit_service::format_percentage_display(machine.scaling_y as f64);
    let default_machine = MachineProfile::default();
    let header_rows = rows_for_template(&default_machine.gcode_header, 6, 18);
    let footer_rows = rows_for_template(&default_machine.gcode_footer, 2, 8);
    let route_plunge_rows = rows_for_template(&default_machine.route_plunge_and_offset, 3, 12);
    let tool_change_rows = rows_for_template(&default_machine.tool_change_command, 4, 12);
    let pending_required_labels = machine
        .pending_required_fields
        .iter()
        .filter_map(|key| cnc_required_field_label(key.as_str()))
        .collect::<Vec<_>>();
    let pending_required_message = if pending_required_labels.is_empty() {
        String::new()
    } else {
        format!(
            "Required values from schema need input: {}",
            pending_required_labels.join(", ")
        )
    };
    let dialog_template_options = if *dialog_is_clone.read() {
        Vec::<(String, String)>::new()
    } else {
        library_profiles
            .iter()
            .map(|profile| (profile.key.clone(), profile.name.clone()))
            .collect::<Vec<_>>()
    };
    let library_profiles_for_add = library_profiles.clone();
    let library_profiles_for_submit = library_profiles.clone();
    let mut cnc_persist_tracking = use_signal(|| None::<(String, String)>);

    // Auto-persist selected CNC profile edits. This covers attribute updates that
    // currently mutate the local UI signal directly.
    use_effect(move || {
        let snapshot = state.read().clone();
        let Some(selected_machine) = snapshot.selected_machine().cloned() else {
            cnc_persist_tracking.set(None);
            return;
        };

        if selected_machine.built_in {
            cnc_persist_tracking.set(None);
            return;
        }

        let selected_id = selected_machine.id.clone();
        let fingerprint = cnc_machine_fingerprint(&selected_machine);
        let tracked = cnc_persist_tracking.read().clone();

        match tracked {
            Some((tracked_id, tracked_fingerprint)) if tracked_id == selected_id => {
                if tracked_fingerprint != fingerprint {
                    persist_cnc_realm_now(state);
                    cnc_persist_tracking.set(Some((selected_id, fingerprint)));
                }
            }
            _ => {
                cnc_persist_tracking.set(Some((selected_id, fingerprint)));
            }
        }
    });

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "CNC profile management" }
                    p {
                        "CNC profiles are editable user profiles. New profiles are created from built-in templates."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "CNC".to_string(),
                    profiles: machine_options,
                    selected_profile_id: snapshot.selected_machine_id.clone(),
                    can_export: has_selected_machine,
                    on_select: move |id| {
                        super::mutate_ctx(state, |s| s.select_machine_profile_by_id(Some(id)));
                    },
                    on_clone: move |_| {
                        let Some(selected) = selected_profile.clone() else {
                            status_message.set("No CNC profile selected".to_string());
                            return;
                        };
                        dialog_is_clone.set(true);
                        dialog_name.set(format!("Copy of {}", selected.name));
                        show_name_dialog.set(true);
                    },
                    on_delete: move |_| {
                        let Some(cnc_id) = state.read().selected_machine_id.clone() else {
                            status_message.set("No CNC profile selected".to_string());
                            return;
                        };
                        let impact = state.read().impact_delete_cnc_profile(&cnc_id);
                        if !impact.dependent_process_profiles.is_empty() {
                            let description = format_impact_warning(
                                "Cannot delete CNC profile because it is referenced by machining profiles:",
                                &impact,
                            );
                            status_message.set(description);
                            return;
                        }
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete CNC profile")
                            .set_description("Delete CNC profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            super::mutate_ctx(
                                state,
                                |s| {
                                    let _ = s.delete_cnc_profile_with_cascade(&cnc_id);
                                    s.log_event("CNC profile deleted");
                                },
                            );
                            status_message.set("CNC profile deleted".to_string());
                        }
                    },
                    on_export: move |_| {
                        let Some(current) = state.read().selected_machine().cloned() else {
                            status_message.set("No CNC profile selected".to_string());
                            return;
                        };
                        let default_name = format!(
                            "{}.cnc-profile.yaml",
                            slug_file_name(&current.name, "cnc-profile"),
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
                        let yaml = match machine_profile_to_yaml(&current) {
                            Ok(v) => v,
                            Err(_) => {
                                status_message
                                    .set("Export failed: unable to serialize profile".to_string());
                                return;
                            }
                        };
                        if fs::write(&output_path, yaml).is_ok() {
                            super::mutate_ctx(state, |s| s.log_event("CNC profile exported"));
                            status_message.set("CNC profile exported".to_string());
                        } else {
                            status_message.set("Export failed: unable to write file".to_string());
                        }
                    },
                    on_add: move |_| {
                        dialog_is_clone.set(false);
                        dialog_name.set("My CNC profile".to_string());
                        let first_template = library_profiles_for_add
                            .first()
                            .map(|profile| profile.key.clone())
                            .unwrap_or_default();
                        selected_template.set(first_template);
                        show_name_dialog.set(true);
                    },
                    on_import: move |_| {
                        let picked = FileDialog::new()
                            .set_title("Import CNC profile")
                            .add_filter("CNC profile YAML", &["yaml", "yml"])
                            .pick_file();
                        let Some(path) = picked else {
                            return;
                        };

                        let file_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        let valid_name = file_name.ends_with(".cnc-profile.yaml")
                            || file_name.ends_with(".cnc-profile.yml");
                        if !valid_name {
                            status_message
                                .set(
                                    "CNC profile import failed: file name must end with .cnc-profile.yaml or .cnc-profile.yml"
                                        .to_string(),
                                );
                            return;
                        }

                        let text = match fs::read_to_string(&path) {
                            Ok(text) => text,
                            Err(_) => {
                                status_message
                                    .set("CNC profile import failed: file not readable".to_string());
                                return;
                            }
                        };

                        let source_path = path.to_string_lossy().to_string();
                        let Some(mut parsed) = parse_machine_profile_yaml(&text, &source_path)
                        else {
                            status_message
                                .set("CNC profile import failed: invalid schema or syntax".to_string());
                            return;
                        };
                        parsed.id = String::new();
                        parsed.built_in = false;
                        if parsed.name.trim().is_empty() {
                            parsed.name = "Imported CNC profile".to_string();
                        }
                        super::mutate_ctx(
                            state,
                            |s| {
                                s.add_machine_profile(parsed);
                                s.log_event("CNC profile imported");
                            },
                        );
                        status_message.set("CNC profile imported and selected".to_string());
                    },
                }
            }

            if !status_message.read().is_empty() {
                p { class: "diag-status", "{status_message}" }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { "Clone CNC profile".to_string() } else { "Add CNC profile".to_string() },
                    name_label: "Profile name".to_string(),
                    name_value: dialog_name.read().clone(),
                    template_options: dialog_template_options,
                    selected_template: selected_template.read().clone(),
                    on_name_change: move |value| dialog_name.set(value),
                    on_template_change: move |value| selected_template.set(value),
                    on_cancel: move |_| show_name_dialog.set(false),
                    on_submit: move |_| {
                        let name = dialog_name.read().trim().to_string();
                        if name.is_empty() {
                            status_message.set("Profile name is required".to_string());
                            return;
                        }
                        if *dialog_is_clone.read() {
                            super::mutate_ctx(state, |s| s.clone_selected_machine());
                            let result = super::mutate_ctx(state, |s| s.rename_selected_machine(&name));
                            if let Err(message) = result {
                                status_message.set(message);
                                return;
                            }
                            super::mutate_ctx(state, |s| s.log_event("CNC profile cloned"));
                            status_message.set("CNC profile cloned".to_string());
                        } else {
                            let selected_key = selected_template.read().clone();
                            let selected_profile = library_profiles_for_submit
                                .iter()
                                .find(|profile| profile.key == selected_key)
                                .cloned();

                            if let Some(template) = selected_profile {
                                super::mutate_ctx(
                                    state,
                                    |s| {
                                        let mut machine = template.machine;
                                        machine.id = String::new();
                                        machine.built_in = false;
                                        machine.name = name.clone();
                                        s.add_machine_profile(machine);
                                        s.log_event("CNC profile added");
                                    },
                                );
                                status_message.set("CNC profile created from template".to_string());
                            } else {
                                super::mutate_ctx(
                                    state,
                                    |s| {
                                        s.add_machine_profile_from_schema(&name);
                                        s.log_event("CNC profile added");
                                    },
                                );
                                status_message.set("CNC profile created".to_string());
                            }
                        }
                        show_name_dialog.set(false);
                    },
                }
            }

            div { class: "panel stock-detail-panel cnc-profile-details-panel profile-editor-shell",
                div { class: "panel-header",
                    h3 {
                        if has_selected_machine {
                            "CNC profile editor: {machine.name}"
                        } else {
                            "CNC profile editor"
                        }
                    }
                }

                if !pending_required_message.is_empty() {
                    p { class: "diag-status required-pending-help", "{pending_required_message}" }
                }

                div { class: "profile-editor-scroll",

                    if !has_selected_profile {
                        p {
                            "No CNC profile selected. Add a profile from a template in the top section."
                        }
                        p { class: "diag-status",
                            "The details editor is read-only until a profile is selected."
                        }
                    }

                    if has_selected_machine && !is_built_in {
                        div { class: if editor_read_only { "edit-grid read-only" } else { "edit-grid" },
                            div { class: "field section-subfield",
                                label { "Profile name" }
                                div { class: "sub-field",
                                    if editor_read_only {
                                        p { class: "diag-status", "{machine.name}" }
                                    } else {
                                        input {
                                            value: machine.name.clone(),
                                            autofocus: snapshot.focus_profile_name_editor,
                                            oninput: move |evt| {
                                                let proposed = evt.value();
                                                super::mutate_ctx(
                                                    state,
                                                    |s| {
                                                        match s.rename_selected_machine(&proposed) {
                                                            Ok(_) => {
                                                                s.focus_profile_name_editor = false;
                                                                status_message.set(String::new());
                                                            }
                                                            Err(msg) => {
                                                                status_message.set(msg);
                                                            }
                                                        }
                                                    },
                                                );
                                            },
                                        }
                                    }
                                }
                            }

                            div { class: if machine.pending_required_fields.contains("machine.atc_slot_count") { "field section-subfield required-pending" } else { "field section-subfield" },
                                label { "ATC slots" }
                                div { class: "sub-field",
                                    input {
                                        r#type: "number",
                                        min: "0",
                                        disabled: editor_read_only,
                                        value: "{machine.atc_slot_count}",
                                        oninput: {
                                            let selected_id = selected_id.clone();
                                            move |evt| {
                                                let value = evt.value().parse::<u8>().unwrap_or(0);
                                                state
                                                    .with_mut(|s| {
                                                        if let Some(t) = s
                                                            .machines
                                                            .iter_mut()
                                                            .find(|m| m.id == selected_id)
                                                        {
                                                            t.atc_slot_count = value;
                                                            t.pending_required_fields.remove("machine.atc_slot_count");
                                                        }
                                                        s.seed_rack_slots(value);
                                                    });
                                            }
                                        },
                                    }
                                }
                            }

                            div { class: "field section-block",
                                if let Some(message) = field_error_message.read().clone() {
                                    p { class: "diag-status", "{message}" }
                                }

                                div { class: if machine.pending_required_fields.contains("machine.max_feed_rate") { "field section-subfield required-pending" } else { "field section-subfield" },
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
                                                            match unit_service::parse_feed_with_preference(&raw, unit_system) {
                                                                Ok(feed_rate) if feed_rate.as_mm_per_min() >= 0.0 => {
                                                                    state
                                                                        .with_mut(|s| {
                                                                            if let Some(t) = s

                                                                                .machines
                                                                                .iter_mut()
                                                                                .find(|m| m.id == selected_id)
                                                                            {
                                                                                t.max_feed_rate = feed_rate;
                                                                                t.pending_required_fields
                                                                                    .remove("machine.max_feed_rate");
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
                                                    let selected_id = selected_id.clone();
                                                    move |_| {
                                                        let raw = feed_draft.read().trim().to_string();
                                                        if let Ok(feed_rate) = unit_service::parse_feed_with_preference(
                                                            &raw,
                                                            unit_system,
                                                        ) {
                                                            if feed_rate.as_mm_per_min() >= 0.0 {
                                                                state
                                                                    .with_mut(|s| {
                                                                        if let Some(t) = s
                                                                            .machines
                                                                            .iter_mut()
                                                                            .find(|m| m.id == selected_id)
                                                                        {
                                                                            t.max_feed_rate = feed_rate;
                                                                            t.pending_required_fields.remove("machine.max_feed_rate");
                                                                        }
                                                                    });
                                                                persist_cnc_realm_now(state);
                                                                field_error_message.set(None);
                                                            } else {
                                                                feed_draft.set(feed_edit_seed.clone());
                                                            }
                                                        } else {
                                                            feed_draft.set(feed_edit_seed.clone());
                                                        }
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

                                div { class: if machine.pending_required_fields.contains("machine.spindle_rpm_min") { "field section-subfield required-pending" } else { "field section-subfield" },
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
                                                                                t.spindle_rpm_min = speed;
                                                                                t.pending_required_fields.remove("machine.spindle_rpm_min");
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

                                div { class: if machine.pending_required_fields.contains("machine.spindle_rpm_max") { "field section-subfield required-pending" } else { "field section-subfield" },
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
                                                                                t.spindle_rpm_max = speed;
                                                                                t.pending_required_fields.remove("machine.spindle_rpm_max");
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
                                h4 { "Axis scaling" }

                                if let Some(message) = scaling_error_message.read().clone() {
                                    p { class: "diag-status", "{message}" }
                                }

                                div { class: if machine.pending_required_fields.contains("machine.scaling.x") { "field section-subfield required-pending" } else { "field section-subfield" },
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
                                                                            t.pending_required_fields.remove("machine.scaling.x");
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

                                div { class: if machine.pending_required_fields.contains("machine.scaling.y") { "field section-subfield required-pending" } else { "field section-subfield" },
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
                                                                            t.pending_required_fields.remove("machine.scaling.y");
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
                                            checked: machine.line_numbering_increment > 0,
                                            oninput: {
                                                let selected_id = selected_id.clone();
                                                move |evt| {
                                                    let enabled = evt.checked();
                                                    state
                                                        .with_mut(|s| {
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
                                                            {
                                                                t.line_numbering_increment = if enabled { 10 } else { 0 };
                                                            }
                                                        });
                                                    persist_cnc_realm_now(state);
                                                }
                                            },
                                        }
                                        span { "Emit line numbers" }
                                    }
                                }

                                if machine.line_numbering_increment > 0 {
                                    div { class: if machine.pending_required_fields.contains("machine.line_numbering_increment") { "field section-subfield required-pending" } else { "field section-subfield" },
                                        label { "Increment" }
                                        input {
                                            r#type: "number",
                                            min: "1",
                                            step: "1",
                                            value: "{machine.line_numbering_increment}",
                                            oninput: {
                                                let selected_id = selected_id.clone();
                                                move |evt| {
                                                    let value = evt.value().parse::<u16>().unwrap_or(10).max(1);
                                                    state
                                                        .with_mut(|s| {
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
                                                            {
                                                                t.line_numbering_increment = value;
                                                                t.pending_required_fields
                                                                    .remove("machine.line_numbering_increment");
                                                            }
                                                        });
                                                    persist_cnc_realm_now(state);
                                                }
                                            },
                                        }
                                    }
                                }
                            }

                            div { class: "field section-block full-width",
                                h4 { "Primitives (schema)" }

                                p { class: "diag-status",
                                    "Use {{placeholders}} where documented. Unknown placeholders are preserved as-is."
                                }

                                div { class: "field section-subfield section-block",
                                    h4 { "Program lifecycle primitives" }

                                    div { class: "field section-subfield",
                                        label { "initialise" }
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
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
                                                            {
                                                                t.gcode_header = value;
                                                            }
                                                        });
                                                }
                                            },
                                        }
                                    }

                                    div { class: "field section-subfield",
                                        label { "conclude" }
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
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
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
                                    h4 { "Motion / spindle / drilling primitives" }

                                    for (lbl , getter , setter) in [
                                        ("rapid_move", machine.drill_first_move.clone(), "drill_first_move"),
                                        ("peck_drill", machine.drill_cycle_mode_last.clone(), "drill_cycle_mode_last"),
                                        (
                                            "linear_cut",
                                            machine.drill_cycle_mode_series.clone(),
                                            "drill_cycle_mode_series",
                                        ),
                                        ("start_spindle", machine.drill_cycle_start.clone(), "drill_cycle_start"),
                                        ("drill", machine.drill_next_hole.clone(), "drill_next_hole"),
                                        ("stop_spindle", machine.drill_cycle_cancel.clone(), "drill_cycle_cancel"),
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
                                                                if let Some(t) = s
                                                                    .machines
                                                                    .iter_mut()
                                                                    .find(|m| m.id == selected_id)
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
                                    h4 { "Arc / bezier and optional primitives" }

                                    div { class: "field section-subfield",
                                        label { "cut_arc" }
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
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
                                                            {
                                                                t.route_plunge_and_offset = value;
                                                            }
                                                        });
                                                }
                                            },
                                        }
                                    }

                                    for (lbl , getter , setter) in [
                                        ("cut_bezier", machine.route_arc_up.clone(), "route_arc_up"),
                                        ("pause (optional)", machine.route_arc_down.clone(), "route_arc_down"),
                                        ("banner (optional)", machine.route_retract.clone(), "route_retract"),
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
                                                                if let Some(t) = s
                                                                    .machines
                                                                    .iter_mut()
                                                                    .find(|m| m.id == selected_id)
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
                                    h4 { "Tool change primitives" }

                                    div { class: "field section-subfield",
                                        label { "change_tool" }
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
                                                            if let Some(t) = s
                                                                .machines
                                                                .iter_mut()
                                                                .find(|m| m.id == selected_id)
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
                }
            }

        }
    }
}

#[derive(Serialize)]
struct ExportScaling {
    x: f32,
    y: f32,
}

#[derive(Serialize)]
struct ExportMachine {
    max_feed_rate: String,
    spindle_rpm_min: String,
    spindle_rpm_max: String,
    atc_slot_count: u8,
    scaling: ExportScaling,
    line_numbering_increment: u16,
}

#[derive(Serialize)]
struct ExportPrimitives {
    use_metric: String,
    use_imperial: String,
    initialise: String,
    rapid_move: String,
    linear_cut: String,
    start_spindle: String,
    stop_spindle: String,
    drill: String,
    peck_drill: String,
    cut_arc: String,
    cut_bezier: String,
    change_tool: String,
    conclude: String,
    pause: String,
    banner: String,
}

#[derive(Serialize)]
struct ExportProfile {
    schema_version: u8,
    id: String,
    machine: ExportMachine,
    primitives: ExportPrimitives,
}

fn machine_profile_to_yaml(machine: &MachineProfile) -> Result<String, serde_yaml::Error> {
    let profile = ExportProfile {
        schema_version: 1,
        id: machine.id.clone(),
        machine: ExportMachine {
            max_feed_rate: machine.max_feed_rate.to_string(),
            spindle_rpm_min: machine.spindle_rpm_min.to_string(),
            spindle_rpm_max: machine.spindle_rpm_max.to_string(),
            atc_slot_count: machine.atc_slot_count,
            scaling: ExportScaling {
                x: machine.scaling_x,
                y: machine.scaling_y,
            },
            line_numbering_increment: machine.line_numbering_increment,
        },
        primitives: ExportPrimitives {
            use_metric: "{set_precision(3)}G21".to_string(),
            use_imperial: "{set_precision(5)}G20".to_string(),
            initialise: machine.gcode_header.clone(),
            rapid_move: machine.drill_first_move.clone(),
            linear_cut: machine.drill_cycle_mode_series.clone(),
            start_spindle: machine.drill_cycle_start.clone(),
            stop_spindle: machine.drill_cycle_cancel.clone(),
            drill: machine.drill_next_hole.clone(),
            peck_drill: machine.drill_cycle_mode_last.clone(),
            cut_arc: machine.route_plunge_and_offset.clone(),
            cut_bezier: machine.route_arc_up.clone(),
            change_tool: machine.tool_change_command.clone(),
            conclude: machine.gcode_footer.clone(),
            pause: machine.route_arc_down.clone(),
            banner: machine.route_retract.clone(),
        },
    };

    serde_yaml::to_string(&profile)
}

fn persist_cnc_realm_now(state: Signal<crate::ctx::AppCtx>) {
    let snapshot = state.read().clone();
    snapshot.persist_realms(&[PersistRealm::CncProfiles]);
}

fn cnc_machine_fingerprint(machine: &MachineProfile) -> String {
    let pending_required = machine
        .pending_required_fields
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("|");

    format!(
        "id={};name={};usable={};built_in={};max_feed={};spindle_min={};spindle_max={};atc={};scaling_x={};scaling_y={};line_inc={};hdr={};ftr={};dfm={};dcml={};dcms={};dcs={};dnh={};dcc={};rpo={};rau={};rad={};rr={};tcc={};pending={}",
        machine.id,
        machine.name,
        machine.usable,
        machine.built_in,
        machine.max_feed_rate,
        machine.spindle_rpm_min,
        machine.spindle_rpm_max,
        machine.atc_slot_count,
        machine.scaling_x,
        machine.scaling_y,
        machine.line_numbering_increment,
        machine.gcode_header,
        machine.gcode_footer,
        machine.drill_first_move,
        machine.drill_cycle_mode_last,
        machine.drill_cycle_mode_series,
        machine.drill_cycle_start,
        machine.drill_next_hole,
        machine.drill_cycle_cancel,
        machine.route_plunge_and_offset,
        machine.route_arc_up,
        machine.route_arc_down,
        machine.route_retract,
        machine.tool_change_command,
        pending_required,
    )
}

fn rows_for_template(text: &str, min_rows: usize, max_rows: usize) -> usize {
    let lines = text.lines().count().max(1);
    lines.clamp(min_rows, max_rows)
}


