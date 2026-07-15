use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::profiles_common::{
    format_impact_warning, slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog,
};

#[component]
pub fn FixtureProfilesScreen(state: Signal<crate::ctx::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My fixture profile".to_string());

    let selected_fixture = snapshot.selected_fixture().cloned();
    let fixture_options = snapshot
        .fixtures
        .iter()
        .map(|fixture| (fixture.id.clone(), fixture.name.clone()))
        .collect::<Vec<_>>();

    rsx! {

        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Fixture profile management" }
                    p {
                        "Fixture profiles describe holding/origin assumptions and are referenced by machining profiles."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Fixture".to_string(),
                    profiles: fixture_options,
                    selected_profile_id: snapshot.selected_fixture_id.clone(),
                    can_export: selected_fixture.is_some(),
                    on_select: move |id| {
                        super::mutate_ctx(state, |s| s.selected_fixture_id = Some(id));
                    },
                    on_clone: move |_| {
                        let Some(selected) = state.read().selected_fixture().cloned() else {
                            status_message.set("No fixture profile selected".to_string());
                            return;
                        };
                        dialog_is_clone.set(true);
                        dialog_name.set(format!("Copy of {}", selected.name));
                        show_name_dialog.set(true);
                    },
                    on_delete: move |_| {
                        let Some(fixture_id) = state.read().selected_fixture_id.clone() else {
                            status_message.set("No fixture profile selected".to_string());
                            return;
                        };
                        let impact = state.read().impact_delete_fixture_profile(&fixture_id);
                        if !impact.dependent_process_profiles.is_empty() {
                            let description = format_impact_warning(
                                "Cannot delete fixture profile because it is referenced by machining profiles:",
                                &impact,
                            );
                            status_message.set(description);
                            return;
                        }
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete fixture profile")
                            .set_description("Delete fixture profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            state
                                .with_mut(|s| {
                                    let _ = s.delete_fixture_profile_with_cascade(&fixture_id);
                                    s.log_event("Fixture profile deleted");
                                });
                            status_message.set("Fixture profile deleted".to_string());
                        }
                    },
                    on_export: move |_| {
                        let Some(current) = state.read().selected_fixture().cloned() else {
                            status_message.set("No fixture profile selected".to_string());
                            return;
                        };

                        let default_name = format!(
                            "{}.fixture-profile.yaml",
                            slug_file_name(&current.name, "fixture-profile"),
                        );
                        let picked = FileDialog::new()
                            .set_title("Export fixture profile")
                            .set_file_name(&default_name)
                            .add_filter("Fixture profile YAML", &["yaml", "yml"])
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
                        if !file_name.ends_with(".fixture-profile.yaml")
                            && !file_name.ends_with(".fixture-profile.yml")
                        {
                            let stem = output_path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("fixture-profile");
                            let new_name = format!("{}.fixture-profile.yaml", stem);
                            output_path = output_path.with_file_name(new_name);
                        }

                        let yaml = match state.read().export_selected_fixture_yaml() {
                            Ok(v) => v,
                            Err(message) => {
                                status_message.set(message);
                                return;
                            }
                        };
                        if fs::write(&output_path, yaml).is_ok() {
                            super::mutate_ctx(state, |s| s.log_event("Fixture profile exported"));
                            status_message.set("Fixture profile exported".to_string());
                        } else {
                            status_message.set("Export failed: unable to write file".to_string());
                        }
                    },
                    on_add: move |_| {
                        dialog_is_clone.set(false);
                        dialog_name.set("My fixture profile".to_string());
                        show_name_dialog.set(true);
                    },
                    on_import: move |_| {
                        let picked = FileDialog::new()
                            .set_title("Import fixture profile")
                            .add_filter("Fixture profile YAML", &["yaml", "yml"])
                            .pick_file();

                        let Some(path) = picked else {
                            return;
                        };

                        let file_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        let valid_name = file_name.ends_with(".fixture-profile.yaml")
                            || file_name.ends_with(".fixture-profile.yml");
                        if !valid_name {
                            status_message
                                .set(
                                    "Fixture profile import failed: file name must end with .fixture-profile.yaml or .fixture-profile.yml"
                                        .to_string(),
                                );
                            return;
                        }

                        let text = match fs::read_to_string(&path) {
                            Ok(text) => text,
                            Err(_) => {
                                status_message
                                    .set("Fixture profile import failed: file not readable".to_string());
                                return;
                            }
                        };
                        let result = super::mutate_ctx(state, |s| s.import_fixture_profile_yaml(&text));
                        match result {
                            Ok(_) => {
                                super::mutate_ctx(state, |s| s.log_event("Fixture profile imported"));
                                status_message.set("Fixture profile imported and selected".to_string())
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
                if let Some(fixture) = selected_fixture.as_ref() {
                    div { class: "profile-editor-top",
                        div { class: "field",
                            label { "Profile name" }
                            input {
                                r#type: "text",
                                value: "{fixture.name}",
                                oninput: move |evt| {
                                    let result = state
                                        .with_mut(|s| { s.rename_selected_fixture_profile(&evt.value()) });
                                    if let Err(message) = result {
                                        status_message.set(message);
                                    }
                                },
                            }
                        }
                    }

                    div { class: "profile-editor-scroll",
                        div { class: "edit-grid",
                            div { class: "field",
                                label { "Board holding method" }
                                input {
                                    r#type: "text",
                                    value: "{fixture.backing_board}",
                                    oninput: move |evt| {
                                        let result = state
                                            .with_mut(|s| { s.update_selected_fixture_backing_board(&evt.value()) });
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: "field",
                                label { "Work origin reference" }
                                input {
                                    r#type: "text",
                                    value: "{fixture.coordinate_context}",
                                    oninput: move |evt| {
                                        let result = state
                                            .with_mut(|s| {
                                                s.update_selected_fixture_coordinate_context(&evt.value())
                                            });
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }
                        }
                    }
                } else {
                    p { class: "diag-status", "Select a fixture profile to edit details." }
                }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { "Clone fixture profile".to_string() } else { "Add fixture profile".to_string() },
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
                                    let result = s.clone_selected_fixture_profile();
                                    if result.is_ok() {
                                        let _ = s.rename_selected_fixture_profile(&name);
                                        s.log_event("Fixture profile cloned");
                                    }
                                    result
                                })
                        } else {
                            state
                                .with_mut(|s| {
                                    s.add_fixture_profile(&name);
                                    s.log_event("Fixture profile added");
                                    Ok(String::new())
                                })
                        };
                        match result {
                            Ok(_) => {
                                status_message
                                    .set(
                                        if *dialog_is_clone.read() {
                                            "Fixture profile cloned".to_string()
                                        } else {
                                            "Fixture profile created".to_string()
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


