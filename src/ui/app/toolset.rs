use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::profiles_common::{
    slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog,
};
use crate::ui::model::ToolStatus;

#[component]
pub fn ToolsetProfilesScreen(state: Signal<crate::app_state_impl::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My toolset profile".to_string());

    let selected_toolset = snapshot.selected_toolset().cloned();
    let toolset_options = snapshot
        .toolsets
        .iter()
        .map(|toolset| {
            let suffix = if toolset.usable { "" } else { " (not usable)" };
            (toolset.id.clone(), format!("{}{}", toolset.name, suffix))
        })
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single stock-shell",
            div { class: "stock-toolbar",
                div {
                    h3 { "Toolset profile management" }
                    p {
                        "Toolset profiles define slot assignment modes and generation policy using the toolset schema."
                    }
                }
                ProfileLifecycleToolbar {
                    profile_type_label: "Toolset".to_string(),
                    profiles: toolset_options,
                    selected_profile_id: snapshot.selected_toolset_id.clone(),
                    can_export: selected_toolset.is_some(),
                    on_select: move |id| {
                        super::mutate_ctx(state, |s| s.select_toolset_profile_by_id(Some(id)));
                    },
                    on_clone: move |_| {
                        let Some(selected) = state.read().selected_toolset().cloned() else {
                            status_message.set("No toolset profile selected".to_string());
                            return;
                        };
                        dialog_is_clone.set(true);
                        dialog_name.set(format!("Copy of {}", selected.name));
                        show_name_dialog.set(true);
                    },
                    on_delete: move |_| {
                        let Some(toolset_id) = state.read().selected_toolset_id.clone() else {
                            status_message.set("No toolset profile selected".to_string());
                            return;
                        };
                        let snapshot = state.read().clone();
                        let toolset_name = snapshot
                            .toolsets
                            .iter()
                            .find(|toolset| toolset.id == toolset_id)
                            .map(|toolset| toolset.name.clone())
                            .unwrap_or_else(|| "toolset".to_string());
                        let referenced_by = snapshot.toolset_referencing_process_profiles(&toolset_id);
                        let description = if referenced_by.is_empty() {
                            "Delete toolset profile?".to_string()
                        } else {
                            let refs = referenced_by
                                .iter()
                                .map(|name| {
                                    format!(
                                        "This Toolset {} is referenced in the Machining {}.",
                                        toolset_name,
                                        name,
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            format!(
                                "{}\n\nDelete anyway? Active references will be kept as broken; non-active references will be cleaned.",
                                refs,
                            )
                        };
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete toolset profile")
                            .set_description(&description)
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            super::mutate_ctx(
                                state,
                                |s| {
                                    let _ = s.delete_toolset_profile_with_cascade(&toolset_id);
                                    s.log_event("Toolset profile deleted");
                                },
                            );
                            status_message.set("Toolset profile deleted".to_string());
                        }
                    },
                    on_export: move |_| {
                        let Some(current) = state.read().selected_toolset().cloned() else {
                            status_message.set("No toolset profile selected".to_string());
                            return;
                        };

                        let default_name = format!(
                            "{}.toolset-profile.yaml",
                            slug_file_name(&current.name, "toolset-profile"),
                        );
                        let picked = FileDialog::new()
                            .set_title("Export toolset profile")
                            .set_file_name(&default_name)
                            .add_filter("Toolset profile YAML", &["yaml", "yml"])
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
                        if !file_name.ends_with(".toolset-profile.yaml")
                            && !file_name.ends_with(".toolset-profile.yml")
                        {
                            let stem = output_path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("toolset-profile");
                            let new_name = format!("{}.toolset-profile.yaml", stem);
                            output_path = output_path.with_file_name(new_name);
                        }

                        let yaml = match state.read().export_selected_toolset_yaml() {
                            Ok(v) => v,
                            Err(message) => {
                                status_message.set(message);
                                return;
                            }
                        };
                        if fs::write(&output_path, yaml).is_ok() {
                            super::mutate_ctx(state, |s| s.log_event("Toolset profile exported"));
                            status_message.set("Toolset profile exported".to_string());
                        } else {
                            status_message.set("Export failed: unable to write file".to_string());
                        }
                    },
                    on_add: move |_| {
                        dialog_is_clone.set(false);
                        dialog_name.set("My toolset profile".to_string());
                        show_name_dialog.set(true);
                    },
                    on_import: move |_| {
                        let picked = FileDialog::new()
                            .set_title("Import toolset profile")
                            .add_filter("Toolset profile YAML", &["yaml", "yml"])
                            .pick_file();

                        let Some(path) = picked else {
                            return;
                        };

                        let file_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                        let valid_name = file_name.ends_with(".toolset-profile.yaml")
                            || file_name.ends_with(".toolset-profile.yml");
                        if !valid_name {
                            status_message
                                .set(
                                    "Toolset profile import failed: file name must end with .toolset-profile.yaml or .toolset-profile.yml"
                                        .to_string(),
                                );
                            return;
                        }

                        let text = match fs::read_to_string(&path) {
                            Ok(text) => text,
                            Err(_) => {
                                status_message
                                    .set("Toolset profile import failed: file not readable".to_string());
                                return;
                            }
                        };

                        let result = super::mutate_ctx(state, |s| s.import_toolset_profile_yaml(&text));
                        match result {
                            Ok(_) => {
                                super::mutate_ctx(state, |s| s.log_event("Toolset profile imported"));
                                status_message.set("Toolset profile imported and selected".to_string());
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
                if let Some(toolset) = selected_toolset.as_ref() {
                    div { class: "profile-editor-top",
                        div { class: if toolset.pending_required_fields.contains("name") { "field required-pending" } else { "field" },
                            label { "Profile name" }
                            input {
                                r#type: "text",
                                value: "{toolset.name}",
                                oninput: move |evt| {
                                    let result = super::mutate_ctx(
                                        state,
                                        |s| s.rename_selected_toolset_profile(&evt.value()),
                                    );
                                    if let Err(message) = result {
                                        status_message.set(message);
                                    }
                                },
                            }
                        }
                    }

                    if !toolset.pending_required_fields.is_empty() {
                        p { class: "diag-status required-pending-help",
                            {
                                format!(
                                    "Required schema values need input: {}",
                                    toolset
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
                            div { class: "field",
                                label { "Description" }
                                input {
                                    r#type: "text",
                                    value: "{toolset.description}",
                                    oninput: move |evt| {
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| s.update_selected_toolset_description(&evt.value()),
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: if toolset.pending_required_fields.contains("generation_policy") { "field required-pending" } else { "field" },
                                label { "Generation policy" }
                                select {
                                    value: toolset.generation_policy.as_key(),
                                    onchange: move |evt| {
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| s.set_selected_toolset_generation_policy(&evt.value()),
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                    option { value: "fixed_toolset", "Fixed toolset" }
                                    option { value: "allow_reload", "Allow reload" }
                                    option { value: "allow_hybrid", "Allow hybrid" }
                                }
                            }

                            div { class: if toolset.pending_required_fields.contains("slots") { "field required-pending" } else { "field" },
                                label { "Slot count" }
                                input {
                                    r#type: "number",
                                    min: "1",
                                    max: "64",
                                    value: "{toolset.slots.len()}",
                                    oninput: move |evt| {
                                        let count = evt.value().parse::<u8>().unwrap_or(1).clamp(1, 64);
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| s.set_selected_toolset_slot_count(count),
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: "field",
                                label { "Slots" }
                                div { class: "profile-list",
                                    for (slot_index , slot) in toolset.slots.iter() {
                                        div {
                                            key: "toolset-slot-{slot_index}",
                                            class: "profile-list-item editable",
                                            div {
                                                div { class: "profile-list-title", "T{slot_index}" }
                                                div { class: "profile-list-meta",
                                                    if slot.disabled {
                                                        "do_not_use"
                                                    } else if slot.locked {
                                                        "fixed"
                                                    } else {
                                                        "spare"
                                                    }
                                                }
                                            }
                                            div { class: "actions",
                                                select {
                                                    class: if slot.disabled { "toolset-slot-select state-do-not-use" } else if slot.locked { "toolset-slot-select state-fixed" } else { "toolset-slot-select state-spare" },
                                                    value: if slot.disabled { "do_not_use".to_string() } else if slot.locked { slot
                                                                                                                                                                                                                        .tool_id
                                                        .as_ref()
                                                        .map(|tool_id| format!("tool:{tool_id}"))
                                                        .unwrap_or_else(|| "spare".to_string()) } else { "spare".to_string() },
                                                    onchange: {
                                                        let idx = *slot_index;
                                                        move |evt| {
                                                            let selected = evt.value();
                                                            let result = if selected == "spare" {
                                                                super::mutate_ctx(
                                                                    state,
                                                                    |s| s.set_selected_toolset_slot_mode(idx, "spare", None),
                                                                )
                                                            } else if selected == "do_not_use" {
                                                                super::mutate_ctx(
                                                                    state,
                                                                    |s| s.set_selected_toolset_slot_mode(idx, "do_not_use", None),
                                                                )
                                                            } else {
                                                                let tool_id = selected.strip_prefix("tool:").map(|v| v.to_string());
                                                                super::mutate_ctx(
                                                                    state,
                                                                    |s| s.set_selected_toolset_slot_mode(idx, "fixed", tool_id),
                                                                )
                                                            };
                                                            if let Err(message) = result {
                                                                status_message.set(message);
                                                            }
                                                        }
                                                    },
                                                    option {
                                                        class: "toolset-slot-option-spare",
                                                        value: "spare",
                                                        selected: !slot.disabled && (!slot.locked || slot.tool_id.is_none()),
                                                        "spare"
                                                    }
                                                    option {
                                                        class: "toolset-slot-option-do-not-use",
                                                        value: "do_not_use",
                                                        selected: slot.disabled,
                                                        "do_not_use"
                                                    }
                                                    if slot.tool_id.is_some()
                                                        && !snapshot
                                                            .tools
                                                            .iter()
                                                            .filter(|tool| tool.status == ToolStatus::InStock)
                                                            .any(|tool| Some(tool.id.as_str()) == slot.tool_id.as_deref())
                                                    {
                                                        option {
                                                            value: "tool:{slot.tool_id.clone().unwrap_or_default()}",
                                                            selected: slot.locked && !slot.disabled,
                                                            "Missing tool ({slot.tool_id.clone().unwrap_or_default()})"
                                                        }
                                                    }
                                                    for tool in snapshot.tools.iter().filter(|tool| tool.status == ToolStatus::InStock) {
                                                        option {
                                                            key: "tool-option-{slot_index}-{tool.id}",
                                                            value: "tool:{tool.id}",
                                                            selected: !slot.disabled
                                                                                                                                                                                                                                                    && slot.locked
                                                                                                                                                                                                                                                    && Some(tool.id.as_str()) == slot.tool_id.as_deref(),
                                                            "{tool.display_name()}"
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
                } else {
                    p { class: "diag-status", "Select a toolset profile to edit details." }
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
                            let result = if *dialog_is_clone.read() {
                                super::mutate_ctx(
                                    state,
                                    |s| {
                                        let result = s.clone_selected_toolset_profile();
                                        if result.is_ok() {
                                            let _ = s.rename_selected_toolset_profile(&name);
                                            s.log_event("Toolset profile cloned");
                                        }
                                        result
                                    },
                                )
                            } else {
                                super::mutate_ctx(
                                    state,
                                    |s| {
                                        s.add_toolset_profile(&name);
                                        s.log_event("Toolset profile added");
                                        Ok(String::new())
                                    },
                                )
                            };
                            match result {
                                Ok(_) => {
                                    status_message
                                        .set(
                                            if *dialog_is_clone.read() {
                                                "Toolset profile cloned".to_string()
                                            } else {
                                                "Toolset profile created".to_string()
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
