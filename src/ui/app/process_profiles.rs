use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::super::model::*;
use super::profiles_common::{
    format_impact_warning, slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog,
};

#[component]
pub fn ProcessProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My processing profile".to_string());

    let selected_process_profile = snapshot.selected_process_profile().cloned();
    let profile_options = snapshot
        .process_profiles
        .iter()
        .map(|profile| (profile.id.clone(), profile.name.clone()))
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single",
            section { class: "panel grow",
                article { class: "setup-card section-block cnc-manager-shell",
                    div { class: "panel-header",
                        div {
                            h3 { "Processing profile management" }
                            p {
                                "Processing profiles bind CNC, fixture, and toolset defaults plus operation defaults."
                            }
                        }
                        ProfileLifecycleToolbar {
                            profile_type_label: "Processing".to_string(),
                            profiles: profile_options,
                            selected_profile_id: snapshot.selected_process_profile_id.clone(),
                            can_export: selected_process_profile.is_some(),
                            on_select: move |id| {
                                state.with_mut(|s| s.select_process_profile_by_id(Some(id)));
                            },
                            on_clone: move |_| {
                                let Some(selected) = state.read().selected_process_profile().cloned() else {
                                    status_message.set("No processing profile selected".to_string());
                                    return;
                                };
                                dialog_is_clone.set(true);
                                dialog_name.set(format!("Copy of {}", selected.name));
                                show_name_dialog.set(true);
                            },
                            on_delete: move |_| {
                                let Some(profile_id) = state.read().selected_process_profile_id.clone() else {
                                    status_message.set("No processing profile selected".to_string());
                                    return;
                                };
                                let impact = state.read().impact_delete_process_profile(&profile_id);
                                let description = format_impact_warning("Delete processing profile?", &impact);
                                let confirmed = MessageDialog::new()
                                    .set_level(MessageLevel::Warning)
                                    .set_title("Delete processing profile")
                                    .set_description(&description)
                                    .set_buttons(MessageButtons::YesNo)
                                    .show();
                                if confirmed == rfd::MessageDialogResult::Yes {
                                    state
                                        .with_mut(|s| {
                                            let _ = s.delete_process_profile_with_cascade(&profile_id);
                                            s.log_event("Processing profile deleted");
                                        });
                                    status_message.set("Processing profile deleted".to_string());
                                }
                            },
                            on_export: move |_| {
                                let Some(profile) = state.read().selected_process_profile().cloned() else {
                                    status_message.set("No processing profile selected".to_string());
                                    return;
                                };
                                let default_name = format!(
                                    "{}.processing-profile.yaml",
                                    slug_file_name(&profile.name, "processing-profile"),
                                );
                                let picked = FileDialog::new()
                                    .set_title("Export processing profile")
                                    .set_file_name(&default_name)
                                    .add_filter("Processing profile YAML", &["yaml", "yml"])
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
                                if !file_name.ends_with(".processing-profile.yaml")
                                    && !file_name.ends_with(".processing-profile.yml")
                                {
                                    let stem = output_path
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("processing-profile");
                                    let new_name = format!("{}.processing-profile.yaml", stem);
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
                                    state.with_mut(|s| s.log_event("Processing profile exported"));
                                    status_message.set("Processing profile exported".to_string());
                                } else {
                                    status_message.set("Export failed: unable to write file".to_string());
                                }
                            },
                            on_add: move |_| {
                                dialog_is_clone.set(false);
                                dialog_name.set("My processing profile".to_string());
                                show_name_dialog.set(true);
                            },
                            on_import: move |_| {
                                let picked = FileDialog::new()
                                    .set_title("Import processing profile")
                                    .add_filter("Processing profile YAML", &["yaml", "yml"])
                                    .pick_file();

                                let Some(path) = picked else {
                                    return;
                                };

                                let file_name = path
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or_default()
                                    .to_ascii_lowercase();
                                let valid_name = file_name.ends_with(".processing-profile.yaml")
                                    || file_name.ends_with(".processing-profile.yml");
                                if !valid_name {
                                    status_message
                                        .set(
                                            "Processing profile import failed: file name must end with .processing-profile.yaml or .processing-profile.yml"
                                                .to_string(),
                                        );
                                    return;
                                }

                                let text = match fs::read_to_string(&path) {
                                    Ok(text) => text,
                                    Err(_) => {
                                        status_message
                                            .set(
                                                "Processing profile import failed: file not readable"
                                                    .to_string(),
                                            );
                                        return;
                                    }
                                };
                                let result = state.with_mut(|s| s.import_process_profile_yaml(&text));
                                match result {
                                    Ok(_) => {
                                        state.with_mut(|s| s.log_event("Processing profile imported"));
                                        status_message
                                            .set("Processing profile imported and selected".to_string())
                                    }
                                    Err(message) => status_message.set(message),
                                }
                            },
                        }
                    }

                    if !status_message.read().is_empty() {
                        p { class: "diag-status", "{status_message}" }
                    }

                    div { class: "setup-card cnc-profile-details-panel",
                        if let Some(profile) = selected_process_profile.as_ref() {
                            div { class: "edit-grid",
                                div { class: "field",
                                    label { "Profile name" }
                                    input {
                                        r#type: "text",
                                        value: "{profile.name}",
                                        oninput: move |evt| {
                                            let value = evt.value();
                                            let result = state.with_mut(|s| s.rename_selected_process_profile(&value));
                                            if let Err(message) = result {
                                                status_message.set(message);
                                            }
                                        },
                                    }
                                }

                                div { class: "field",
                                    label { "CNC profile" }
                                    select {
                                        value: "{profile.cnc_profile_id}",
                                        onchange: move |evt| {
                                            let value = evt.value();
                                            let result = state.with_mut(|s| s.set_selected_process_profile_cnc(&value));
                                            if let Err(message) = result {
                                                status_message.set(message);
                                            }
                                        },
                                        for machine in snapshot.machines.iter() {
                                            option { value: "{machine.id}", "{machine.name}" }
                                        }
                                    }
                                }

                                div { class: "field",
                                    label { "Fixture profile" }
                                    select {
                                        value: "{profile.fixture_profile_id}",
                                        onchange: move |evt| {
                                            let value = evt.value();
                                            let result = state.with_mut(|s| s.set_selected_process_profile_fixture(&value));
                                            if let Err(message) = result {
                                                status_message.set(message);
                                            }
                                        },
                                        for fixture in snapshot.fixtures.iter() {
                                            option { value: "{fixture.id}", "{fixture.name}" }
                                        }
                                    }
                                }

                                div { class: "field",
                                    label { "Toolset profile" }
                                    select {
                                        value: "{profile.toolset_profile_id}",
                                        onchange: move |evt| {
                                            let value = evt.value();
                                            let result = state.with_mut(|s| s.set_selected_process_profile_toolset(&value));
                                            if let Err(message) = result {
                                                status_message.set(message);
                                            }
                                        },
                                        for toolset in snapshot.toolsets.iter() {
                                            option { value: "{toolset.id}", "{toolset.name}" }
                                        }
                                    }
                                }

                                div { class: "field",
                                    label { "Default operations" }
                                    for op in ProductionOperation::all().iter() {
                                        label { class: "checkbox-line",
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
                        } else {
                            p { class: "diag-status", "Select a processing profile to edit details." }
                        }
                    }
                }

                if *show_name_dialog.read() {
                    ProfileNameDialog {
                        title: if *dialog_is_clone.read() { "Clone processing profile".to_string() } else { "Add processing profile".to_string() },
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
                                            s.log_event("Processing profile cloned");
                                        }
                                        result
                                    })
                            } else {
                                state
                                    .with_mut(|s| {
                                        s.add_process_profile(&name);
                                        s.log_event("Processing profile added");
                                        Ok(String::new())
                                    })
                            };
                            match result {
                                Ok(_) => {
                                    status_message
                                        .set(
                                            if *dialog_is_clone.read() {
                                                "Processing profile cloned".to_string()
                                            } else {
                                                "Processing profile created".to_string()
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
}
