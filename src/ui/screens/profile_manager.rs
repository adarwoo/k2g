//! Reusable profile-management shell.
//!
//! Every profile kind (CNC, fixture, toolset, machining) shares the same
//! lifecycle — list, select, add (CNC additionally from a template), clone,
//! delete, import, export — and a detail editor that is just a set of
//! [`SchemaField`]s. `ProfileManager` factors all of that out; a screen becomes
//! a thin wrapper that supplies the kind, labels, the detail field layout, any
//! templates, and an optional delete-safety guard.

use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;
use uuid::Uuid;

use super::profiles_common::{slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog};
use crate::data::Profile;
use crate::ui::bindings::{
    clone_named, create_named, create_named_from_template, export_yaml, import_yaml,
    remove_profile_result, use_profiles, SchemaField,
};

/// A titled group of schema field pointers rendered as one section of the
/// detail editor. An empty `title` renders no header.
#[derive(Clone, PartialEq)]
pub struct FieldGroup {
    pub title: String,
    pub fields: Vec<String>,
}

impl FieldGroup {
    /// A single untitled group of fields — the common case.
    pub fn flat(fields: &[&str]) -> Vec<FieldGroup> {
        vec![FieldGroup {
            title: String::new(),
            fields: fields.iter().map(|s| s.to_string()).collect(),
        }]
    }
}

/// The shared profile-management screen body.
///
/// - `kind` selects the collection.
/// - `type_label` names it in the UI ("Fixture", "CNC", …).
/// - `file_kind` is the export/import filename tag ("fixture-profile").
/// - `groups` lays out the detail editor.
/// - `templates` are `(key, label)` seeds shown in the add dialog (CNC only).
/// - `delete_guard`, if set, is called with the id before delete and may return
///   a message to block it (transitional legacy cross-reference safety).
#[component]
pub fn ProfileManager(
    kind: Profile,
    type_label: String,
    file_kind: String,
    groups: Vec<FieldGroup>,
    templates: Vec<(String, String)>,
    delete_guard: Option<Callback<String, Option<String>>>,
) -> Element {
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| format!("My {} profile", type_label.to_lowercase()));
    let default_template = templates.first().map(|(k, _)| k.clone()).unwrap_or_default();
    let mut selected_template = use_signal(|| default_template);
    let mut selected = use_signal(|| None::<Uuid>);

    let has_templates = !templates.is_empty();
    let profiles = use_profiles(kind);
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
                    h3 { "{type_label} profile management" }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: type_label.clone(),
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
                        if let Some(guard) = delete_guard {
                            if let Some(message) = guard.call(id.to_string()) {
                                status_message.set(message);
                                return;
                            }
                        }
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete profile")
                            .set_description("Delete this profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            match remove_profile_result(id) {
                                Ok(()) => {
                                    selected.set(None);
                                    status_message.set("Profile deleted".to_string());
                                }
                                Err(message) => status_message.set(message),
                            }
                        }
                    },
                    on_export: {
                        let current_name = current_name.clone();
                        let file_kind = file_kind.clone();
                        move |_| {
                            let Some(id) = current else {
                                status_message.set("No profile selected".to_string());
                                return;
                            };
                            let name = current_name.clone().unwrap_or_else(|| file_kind.clone());
                            let default_name = format!("{}.{}.yaml", slug_file_name(&name, &file_kind), file_kind);
                            let Some(path) = FileDialog::new()
                                .set_title("Export profile")
                                .set_file_name(&default_name)
                                .add_filter("Profile YAML", &["yaml", "yml"])
                                .save_file()
                            else {
                                return;
                            };
                            match export_yaml(id) {
                                Some(yaml) => {
                                    if fs::write(&path, yaml).is_ok() {
                                        status_message.set("Profile exported".to_string());
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
                            .set_title("Import profile")
                            .add_filter("Profile YAML", &["yaml", "yml"])
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
                        match import_yaml(kind, &text) {
                            Some(id) => {
                                selected.set(Some(id));
                                status_message.set("Profile imported and selected".to_string());
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
                        for group in groups.clone() {
                            if !group.title.is_empty() {
                                h4 { class: "section-title", "{group.title}" }
                            }
                            div { class: "edit-grid",
                                for ptr in group.fields.clone() {
                                    SchemaField { id, ptr }
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "panel stock-detail-panel profile-editor-shell",
                    p { class: "diag-status", "Select or add a {type_label} profile to edit details." }
                }
            }

            if *show_name_dialog.read() {
                ProfileNameDialog {
                    title: if *dialog_is_clone.read() { format!("Clone {type_label} profile") } else { format!("Add {type_label} profile") },
                    name_label: "Profile name".to_string(),
                    name_value: dialog_name.read().clone(),
                    template_options: if *dialog_is_clone.read() { Vec::new() } else { templates.clone() },
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
                        let is_clone = *dialog_is_clone.read();
                        let result = if is_clone {
                            current.and_then(|id| clone_named(id, &name))
                        } else if has_templates {
                            create_named_from_template(kind, &selected_template.read(), &name)
                        } else {
                            create_named(kind, &name)
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
