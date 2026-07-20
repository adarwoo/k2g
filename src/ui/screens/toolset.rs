use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;
use uuid::Uuid;

use super::profiles_common::{slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog};
use crate::data::Profile;
use crate::ui::bindings::{
    clone_named, create_named, data_revision, export_yaml, import_yaml, refresh_legacy_toolsets,
    remove_profile_result, use_profiles, RackGrid, SchemaField,
};
use crate::data::model::stock::ToolStatus;

/// Toolset ("rack") profile screen, backed by the `AppData` datastore.
///
/// Identity and the generation policy render through [`SchemaField`]; the `T1..Tn`
/// rack renders through [`RackGrid`]. AppData owns the `toolset_profiles` files;
/// the legacy generator still reads the in-memory `toolsets`/`rack_slots`, so the
/// screen mirrors AppData back into that projection on every change (see
/// [`refresh_legacy_toolsets`]). Deletion is guarded natively by the datastore: a
/// toolset referenced by a machining profile refuses to delete, so the user
/// clears the reference first.
#[component]
pub fn ToolsetProfilesScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    use_effect(move || {
        let _ = data_revision();
        refresh_legacy_toolsets();
        state.set(crate::runtime::ctx_snapshot());
    });

    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My toolset profile".to_string());
    let mut selected = use_signal(|| None::<Uuid>);

    let profiles = use_profiles(Profile::Toolset);
    let current = (*selected.read()).or_else(|| profiles.first().map(|(id, _)| *id));
    let current_name = current
        .and_then(|id| profiles.iter().find(|(pid, _)| *pid == id).map(|(_, n)| n.clone()));
    let toolbar_profiles = profiles
        .iter()
        .map(|(id, name)| (id.to_string(), name.clone()))
        .collect::<Vec<_>>();

    // In-stock tools for the rack picker (stock is not on the datastore yet, so
    // the options come from the legacy snapshot).
    let tools = state
        .read()
        .tools
        .iter()
        .filter(|tool| tool.status == ToolStatus::InStock)
        .map(|tool| (tool.id.clone(), tool.display_name()))
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Toolset profile management" }
                    p {
                        "A toolset profile defines the T1..Tn rack \u{2014} each slot fixed to a tool, left spare, or disabled \u{2014} plus the generation policy."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Toolset".to_string(),
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
                            .set_title("Delete toolset profile")
                            .set_description("Delete this toolset profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            match remove_profile_result(id) {
                                Ok(()) => {
                                    selected.set(None);
                                    status_message.set("Toolset profile deleted".to_string());
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
                            let name = current_name.clone().unwrap_or_else(|| "toolset-profile".to_string());
                            let default_name = format!(
                                "{}.toolset-profile.yaml",
                                slug_file_name(&name, "toolset-profile"),
                            );
                            let Some(path) = FileDialog::new()
                                .set_title("Export toolset profile")
                                .set_file_name(&default_name)
                                .add_filter("Toolset profile YAML", &["yaml", "yml"])
                                .save_file()
                            else {
                                return;
                            };
                            match export_yaml(id) {
                                Some(yaml) => {
                                    if fs::write(&path, yaml).is_ok() {
                                        status_message.set("Toolset profile exported".to_string());
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
                            .set_title("Import toolset profile")
                            .add_filter("Toolset profile YAML", &["yaml", "yml"])
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
                        match import_yaml(Profile::Toolset, &text) {
                            Some(id) => {
                                selected.set(Some(id));
                                status_message.set("Toolset profile imported and selected".to_string());
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
                div { class: "panel stock-detail-panel cnc-profile-details-panel profile-editor-shell",
                    div { class: "profile-editor-scroll",
                        div { class: "edit-grid",
                            SchemaField { id, ptr: "/name".to_string() }
                            SchemaField { id, ptr: "/description".to_string() }
                            SchemaField { id, ptr: "/generation_policy".to_string() }
                            RackGrid { id, tools }
                        }
                    }
                }
            } else {
                div { class: "panel stock-detail-panel profile-editor-shell",
                    p { class: "diag-status", "Select or add a toolset profile to edit details." }
                }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { "Clone toolset profile".to_string() } else { "Add toolset profile".to_string() },
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
                            create_named(Profile::Toolset, &name)
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
