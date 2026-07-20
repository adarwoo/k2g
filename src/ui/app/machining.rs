use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;
use uuid::Uuid;

use super::profiles_common::{slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog};
use crate::data::Profile;
use crate::ui::data_bind::{
    clone_named, create_named, data_revision, export_yaml, import_yaml, machining_operations,
    refresh_legacy_machining, remove_profile_result, use_operations, use_profiles, BindingPicker,
    OperationsEditor, SchemaField, SchemaForm,
};

/// Machining ("process") profile screen, fully backed by the `AppData` datastore.
///
/// A machining profile is mostly references (cnc/fixture/toolset bindings) plus an
/// operation set and per-operation configuration. The detail editor is generated
/// from `machining.yaml`: the deep per-operation config renders through
/// [`SchemaForm`]; only the reference bindings and the operation toggles use
/// dedicated pickers. AppData owns the `processing_profiles` files; because the
/// legacy generator still reads the in-memory `process_profiles`, the screen
/// mirrors AppData back into that projection on every change (see
/// [`refresh_legacy_machining`]). Deletion is by simple reference guard — a
/// referenced cnc/fixture/toolset blocks its own deletion, so profiles are
/// removed leaf-first; nothing references a machining profile, so it deletes
/// freely.
#[component]
pub fn MachiningProfilesScreen(state: Signal<crate::app_state_impl::AppCtx>) -> Element {
    // Mirror AppData into the legacy projection on every store mutation, then
    // refresh the legacy snapshot so the generator and other screens agree.
    use_effect(move || {
        let _ = data_revision();
        refresh_legacy_machining();
        state.set(crate::app_state_impl::ctx_snapshot());
    });

    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My machining profile".to_string());
    let mut selected = use_signal(|| None::<Uuid>);

    let profiles = use_profiles(Profile::Machining);
    let current = (*selected.read()).or_else(|| profiles.first().map(|(id, _)| *id));
    let current_name = current
        .and_then(|id| profiles.iter().find(|(pid, _)| *pid == id).map(|(_, n)| n.clone()));
    let toolbar_profiles = profiles
        .iter()
        .map(|(id, name)| (id.to_string(), name.clone()))
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Machining profile management" }
                    p {
                        "A machining profile defines a job context: which CNC, fixture and toolset to use, and which operations to run."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Machining".to_string(),
                    profiles: toolbar_profiles,
                    selected_profile_id: current.map(|id| id.to_string()),
                    can_export: current.is_some(),
                    on_select: move |id: String| selected.set(Uuid::parse_str(&id).ok()),
                    on_add: move |_| {
                        dialog_is_clone.set(false);
                        dialog_name.set(String::new());
                        show_name_dialog.set(true);
                    },
                    on_clone: {
                        let current_name = current_name.clone();
                        move |_| {
                            if current.is_none() {
                                status_message.set("No profile selected".to_string());
                                return;
                            }
                            dialog_is_clone.set(true);
                            dialog_name.set(format!("Copy of {}", current_name.clone().unwrap_or_default()));
                            show_name_dialog.set(true);
                        }
                    },
                    on_delete: move |_| {
                        let Some(id) = current else {
                            status_message.set("No profile selected".to_string());
                            return;
                        };
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete machining profile")
                            .set_description("Delete this machining profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            match remove_profile_result(id) {
                                Ok(()) => {
                                    selected.set(None);
                                    status_message.set("Machining profile deleted".to_string());
                                }
                                Err(message) => status_message.set(message),
                            }
                        }
                    },
                    on_export: {
                        let current_name = current_name.clone();
                        move |_| {
                            let Some(id) = current else {
                                status_message.set("No profile selected".to_string());
                                return;
                            };
                            let name = current_name.clone().unwrap_or_else(|| "machining-profile".to_string());
                            let default_name = format!(
                                "{}.machining-profile.yaml",
                                slug_file_name(&name, "machining-profile"),
                            );
                            let Some(path) = FileDialog::new()
                                .set_title("Export machining profile")
                                .set_file_name(&default_name)
                                .add_filter("Machining profile YAML", &["yaml", "yml"])
                                .save_file()
                            else {
                                return;
                            };
                            match export_yaml(id) {
                                Some(yaml) => {
                                    if fs::write(&path, yaml).is_ok() {
                                        status_message.set("Machining profile exported".to_string());
                                    } else {
                                        status_message.set("Export failed: unable to write file".to_string());
                                    }
                                }
                                None => status_message.set("Export failed".to_string()),
                            }
                        }
                    },
                    on_import: move |_| {
                        let Some(path) = FileDialog::new()
                            .set_title("Import machining profile")
                            .add_filter("Machining profile YAML", &["yaml", "yml"])
                            .pick_file()
                        else {
                            return;
                        };
                        let text = match fs::read_to_string(&path) {
                            Ok(text) => text,
                            Err(_) => {
                                status_message.set("Import failed: file not readable".to_string());
                                return;
                            }
                        };
                        match import_yaml(Profile::Machining, &text) {
                            Some(id) => {
                                selected.set(Some(id));
                                status_message.set("Machining profile imported and selected".to_string());
                            }
                            None => status_message.set("Import failed: invalid profile".to_string()),
                        }
                    },
                }
            }

            if !status_message.read().is_empty() {
                p { class: "diag-status", "{status_message}" }
            }

            if let Some(id) = current {
                MachiningDetail { id }
            } else {
                div { class: "panel stock-detail-panel profile-editor-shell",
                    p { class: "diag-status", "Select or add a machining profile to edit details." }
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
                        let is_clone = *dialog_is_clone.read();
                        let result = if is_clone {
                            current.and_then(|id| clone_named(id, &name))
                        } else {
                            create_named(Profile::Machining, &name)
                        };
                        match result {
                            Some(id) => {
                                selected.set(Some(id));
                                show_name_dialog.set(false);
                                status_message.set(
                                    if is_clone { "Profile cloned".to_string() } else { "Profile created".to_string() },
                                );
                            }
                            None => status_message.set("Operation failed".to_string()),
                        }
                    },
                }
            }
        }
    }
}

/// The machining detail editor: identity + reference bindings + operation set,
/// then schema-generated configuration sections for routing and each enabled
/// operation.
#[component]
fn MachiningDetail(id: Uuid) -> Element {
    let enabled_ops = use_operations(id);

    rsx! {
        div { class: "panel stock-detail-panel cnc-profile-details-panel profile-editor-shell",
            div { class: "profile-editor-scroll",
                div { class: "edit-grid",
                    SchemaField { id, ptr: "/name".to_string() }

                    BindingPicker { id, field: "cnc".to_string(), kind: Profile::Cnc, label: "CNC profile".to_string() }
                    BindingPicker { id, field: "fixture".to_string(), kind: Profile::Fixture, label: "Fixture profile".to_string() }
                    BindingPicker { id, field: "toolset".to_string(), kind: Profile::Toolset, label: "Toolset profile".to_string() }

                    OperationsEditor { id }

                    SchemaField { id, ptr: "/side_to_machine".to_string() }

                    div { class: "schema-section",
                        h4 { class: "section-title", "Routing" }
                        SchemaForm { id, ptr: "/routing".to_string() }
                    }

                    // Configuration sections for the currently enabled operations.
                    for (key , op_label) in machining_operations().iter().copied() {
                        if enabled_ops.iter().any(|op| op == key) {
                            div { class: "schema-section",
                                h4 { class: "section-title", "{op_label}" }
                                if key == "drill_locating_pins" {
                                    p { class: "field-hint", "No additional options." }
                                } else {
                                    SchemaForm { id, ptr: format!("/{key}") }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
