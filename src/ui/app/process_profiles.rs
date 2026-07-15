use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::super::model::*;
use super::profiles_common::{
    format_impact_warning, slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog,
};

#[component]
pub fn MachiningProfilesScreen(state: Signal<crate::ctx::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My machining profile".to_string());

    let selected_machining_profile = snapshot.selected_process_profile().cloned();
    let profile_options = snapshot
        .process_profiles
        .iter()
        .map(|profile| {
            let suffix = if profile.usable { "" } else { " (not usable)" };
            (profile.id.clone(), format!("{}{}", profile.name, suffix))
        })
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Machining profile management" }
                    p {
                        "Machining profiles define a job context. A job can include multiple machining steps, each with a start and an end."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Machining".to_string(),
                    profiles: profile_options,
                    selected_profile_id: snapshot.selected_process_profile_id.clone(),
                    can_export: selected_machining_profile.is_some(),
                    on_select: move |id: String| {
                        state
                            .with_mut(|s| {
                                s.select_process_profile_by_id(Some(id.clone()));
                                s.mark_last_edited_process_profile(Some(id));
                            });
                    },
                    on_clone: move |_| {
                        let Some(selected) = state.read().selected_process_profile().cloned() else {
                            status_message.set("No machining profile selected".to_string());
                            return;
                        };
                        dialog_is_clone.set(true);
                        dialog_name.set(format!("Copy of {}", selected.name));
                        show_name_dialog.set(true);
                    },
                    on_delete: move |_| {
                        let Some(profile_id) = state.read().selected_process_profile_id.clone() else {
                            status_message.set("No machining profile selected".to_string());
                            return;
                        };
                        let impact = state.read().impact_delete_process_profile(&profile_id);
                        let description = format_impact_warning("Delete machining profile?", &impact);
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete machining profile")
                            .set_description(&description)
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            state
                                .with_mut(|s| {
                                    let _ = s.delete_process_profile_with_cascade(&profile_id);
                                    s.log_event("Machining profile deleted");
                                });
                            status_message.set("Machining profile deleted".to_string());
                        }
                    },
                    on_export: move |_| {
                        let Some(profile) = state.read().selected_process_profile().cloned() else {
                            status_message.set("No machining profile selected".to_string());
                            return;
                        };
                        let default_name = format!(
                            "{}.machining-profile.yaml",
                            slug_file_name(&profile.name, "machining-profile"),
                        );
                        let picked = FileDialog::new()
                            .set_title("Export machining profile")
                            .set_file_name(&default_name)
                            .add_filter("Machining profile YAML", &["yaml", "yml"])
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
                        if !file_name.ends_with(".machining-profile.yaml")
                            && !file_name.ends_with(".machining-profile.yml")
                        {
                            let stem = output_path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("machining-profile");
                            let new_name = format!("{}.machining-profile.yaml", stem);
                            output_path = output_path.with_file_name(new_name);
                        }

                        let yaml = match state.read().export_selected_process_profile_yaml() {
                            Ok(v) => v,
                            Err(message) => {
                                status_message.set(message);
                                return;
                            }
                        };
                        if fs::write(&output_path, yaml).is_ok() {
                            super::mutate_ctx(state, |s| s.log_event("Machining profile exported"));
                            status_message.set("Machining profile exported".to_string());
                        } else {
                            status_message.set("Export failed: unable to write file".to_string());
                        }
                    },
                    on_add: move |_| {
                        dialog_is_clone.set(false);
                        dialog_name.set("My machining profile".to_string());
                        show_name_dialog.set(true);
                    },
                    on_import: move |_| {
                        let picked = FileDialog::new()
                            .set_title("Import machining profile")
                            .add_filter("Machining profile YAML", &["yaml", "yml"])
                            .pick_file();

                        let Some(path) = picked else {
                            return;
                        };

                        let file_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        let valid_name = file_name.ends_with(".machining-profile.yaml")
                            || file_name.ends_with(".machining-profile.yml")
                            || file_name.ends_with(".processing-profile.yaml")
                            || file_name.ends_with(".processing-profile.yml");
                        if !valid_name {
                            status_message
                                .set(
                                    "Machining profile import failed: file name must end with .machining-profile.yaml/.yml (or legacy .processing-profile.yaml/.yml)"
                                        .to_string(),
                                );
                            return;
                        }

                        let text = match fs::read_to_string(&path) {
                            Ok(text) => text,
                            Err(_) => {
                                status_message
                                    .set(
                                        "Machining profile import failed: file not readable"
                                            .to_string(),
                                    );
                                return;
                            }
                        };
                        let result = super::mutate_ctx(state, |s| s.import_process_profile_yaml(&text));
                        match result {
                            Ok(_) => {
                                super::mutate_ctx(state, |s| s.log_event("Machining profile imported"));
                                status_message
                                    .set("Machining profile imported and selected".to_string())
                            }
                            Err(message) => status_message.set(message),
                        }
                    },
                }
            }

            if !status_message.read().is_empty() {
                p { class: "diag-status", "{status_message}" }
            }

            div { class: "panel stock-detail-panel cnc-profile-details-panel profile-editor-shell",
                if let Some(profile) = selected_machining_profile.as_ref() {
                    div { class: "profile-editor-top",
                        div { class: if profile.pending_required_fields.contains("name") { "field required-pending" } else { "field" },
                            label { "Profile name" }
                            input {
                                r#type: "text",
                                value: "{profile.name}",
                                oninput: move |evt| {
                                    let value = evt.value();
                                    let result = super::mutate_ctx(
                                        state,
                                        |s| s.rename_selected_process_profile(&value),
                                    );
                                    if let Err(message) = result {
                                        status_message.set(message);
                                    }
                                },
                            }
                        }
                    }

                    if !profile.pending_required_fields.is_empty() {
                        p { class: "diag-status required-pending-help",
                            {
                                format!(
                                    "Required schema values need input: {}",
                                    profile
                                        .pending_required_fields
                                        .iter()
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                )
                            }
                        }
                    }

                    div { class: "profile-editor-scroll",
                        div { class: "edit-grid",
                            div { class: if profile.pending_required_fields.contains("cnc.default") { "field required-pending" } else { "field" },
                                label { "CNC profile" }
                                select {
                                    value: "{profile.cnc_profile_id}",
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| s.set_selected_process_profile_cnc(&value),
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                    for (idx , machine) in snapshot.machines.iter().enumerate() {
                                        option {
                                            key: "mach-opt-{idx}",
                                            value: "{machine.id}",
                                            {format!("{}{}", machine.name, if machine.usable { "" } else { " (not usable)" })}
                                        }
                                    }
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("fixture.default") { "field required-pending" } else { "field" },
                                label { "Fixture profile" }
                                select {
                                    value: "{profile.fixture_profile_id}",
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        let result = state
                                            .with_mut(|s| s.set_selected_process_profile_fixture(&value));
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                    for (idx , fixture) in snapshot.fixtures.iter().enumerate() {
                                        option {
                                            key: "fix-opt-{idx}",
                                            value: "{fixture.id}",
                                            {format!("{}{}", fixture.name, if fixture.usable { "" } else { " (not usable)" })}
                                        }
                                    }
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("toolset.default") { "field required-pending" } else { "field" },
                                label { "Toolset profile" }
                                select {
                                    value: "{profile.toolset_profile_id}",
                                    onchange: move |evt| {
                                        let value = evt.value();
                                        let result = state
                                            .with_mut(|s| s.set_selected_process_profile_toolset(&value));
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                    for (idx , toolset) in snapshot.toolsets.iter().enumerate() {
                                        option {
                                            key: "tool-opt-{idx}",
                                            value: "{toolset.id}",
                                            {format!("{}{}", toolset.name, if toolset.usable { "" } else { " (not usable)" })}
                                        }
                                    }
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("operations") { "field required-pending" } else { "field" },
                                label { "Default machining steps" }
                                for (idx , op) in ProductionOperation::all().iter().enumerate() {
                                    label {
                                        key: "op-{idx}",
                                        class: "checkbox-line",
                                        input {
                                            r#type: "checkbox",
                                            checked: profile.default_operations.contains(op),
                                            oninput: {
                                                let operation = *op;
                                                move |_| {
                                                    let result = state
                                                        .with_mut(|s| {
                                                            s.toggle_selected_process_profile_operation(operation)
                                                        });
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                }
                                            },
                                        }
                                        span { "{op.label()}" }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { class: "diag-status", "Select a machining profile to edit details." }
                }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { "Clone machining profile".to_string() } else { "Add machining profile".to_string() },
                    name_label: "Profile name".to_string(),
                    name_value: dialog_name.read().clone(),
                    template_options: Vec::<(String, String)>::new(),
                    selected_template: String::new(),
                    on_name_change: move |value| dialog_name.set(value),
                    on_template_change: |_| {},
                    on_cancel: move |_| show_name_dialog.set(false),
                    on_submit: move |_| {
                        let name = dialog_name.read().trim().to_string();
                        if name.is_empty() {
                            status_message.set("Profile name is required".to_string());
                            return;
                        }
                        let result = if *dialog_is_clone.read() {
                            state
                                .with_mut(|s| {
                                    let result = s.clone_selected_process_profile();
                                    if result.is_ok() {
                                        let _ = s.rename_selected_process_profile(&name);
                                        s.log_event("Machining profile cloned");
                                    }
                                    result
                                })
                        } else {
                            state
                                .with_mut(|s| {
                                    s.add_process_profile(&name);
                                    s.log_event("Machining profile added");
                                    Ok(String::new())
                                })
                        };
                        match result {
                            Ok(_) => {
                                status_message
                                    .set(
                                        if *dialog_is_clone.read() {
                                            "Machining profile cloned".to_string()
                                        } else {
                                            "Machining profile created".to_string()
                                        },
                                    );
                                show_name_dialog.set(false);
                            }
                            Err(message) => status_message.set(message),
                        }
                    },
                }
            }
        }
    }
}
