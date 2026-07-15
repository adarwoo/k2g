use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::profiles_common::{
    format_impact_warning, slug_file_name, ProfileLifecycleToolbar, ProfileNameDialog,
};

#[component]
pub fn ToolsetProfilesScreen(state: Signal<crate::ctx::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut show_name_dialog = use_signal(|| false);
    let mut dialog_is_clone = use_signal(|| false);
    let mut dialog_name = use_signal(|| "My toolset profile".to_string());

    let selected_toolset = snapshot.selected_toolset().cloned();
    let toolset_options = snapshot
        .toolsets
        .iter()
        .map(|toolset| (toolset.id.clone(), toolset.name.clone()))
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
                        let impact = state.read().impact_delete_toolset_profile(&toolset_id);
                        if !impact.dependent_process_profiles.is_empty() {
                            let description = format_impact_warning(
                                "Cannot delete toolset profile because it is referenced by machining profiles:",
                                &impact,
                            );
                            status_message.set(description);
                            return;
                        }
                        let confirmed = MessageDialog::new()
                            .set_level(MessageLevel::Warning)
                            .set_title("Delete toolset profile")
                            .set_description("Delete toolset profile?")
                            .set_buttons(MessageButtons::YesNo)
                            .show();
                        if confirmed == rfd::MessageDialogResult::Yes {
                            state
                                .with_mut(|s| {
                                    let _ = s.delete_toolset_profile_with_cascade(&toolset_id);
                                    s.log_event("Toolset profile deleted");
                                });
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
                        div { class: "field",
                            label { "Profile name" }
                            input {
                                r#type: "text",
                                value: "{toolset.name}",
                                oninput: move |evt| {
                                    let result = state
                                        .with_mut(|s| { s.rename_selected_toolset_profile(&evt.value()) });
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
                                label { "Description" }
                                input {
                                    r#type: "text",
                                    value: "{toolset.description}",
                                    oninput: move |evt| {
                                        let result = state
                                            .with_mut(|s| { s.update_selected_toolset_description(&evt.value()) });
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: "field",
                                label { "Generation policy" }
                                select {
                                    value: toolset.generation_policy.as_key(),
                                    onchange: move |evt| {
                                        let result = state
                                            .with_mut(|s| { s.set_selected_toolset_generation_policy(&evt.value()) });
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                    option { value: "fixed_toolset", "Fixed toolset" }
                                    option { value: "allow_reload", "Allow reload" }
                                    option { value: "allow_hybrid", "Allow hybrid" }
                                }
                            }

                            div { class: "field",
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
                                        div { class: "profile-list-item editable",
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
                                                    value: if slot.disabled { "do_not_use" } else if slot.locked { "fixed" } else { "spare" },
                                                    onchange: {
                                                        let idx = *slot_index;
                                                        let current_tool = slot.tool_id.clone();
                                                        move |evt| {
                                                            let mode = evt.value();
                                                            let tool_id = if mode == "fixed" {
                                                                current_tool
                                                                    .clone()
                                                                    .or_else(|| {
                                                                        state.read().tools.first().map(|tool| tool.id.clone())
                                                                    })
                                                            } else {
                                                                None
                                                            };
                                                            let result = state
                                                                .with_mut(|s| { s.set_selected_toolset_slot_mode(idx, &mode, tool_id) });
                                                            if let Err(message) = result {
                                                                status_message.set(message);
                                                            }
                                                        }
                                                    },
                                                    option { value: "spare", "spare" }
                                                    option { value: "fixed", "fixed" }
                                                    option { value: "do_not_use", "do_not_use" }
                                                }
                                                if !slot.disabled {
                                                    select {
                                                        disabled: !slot.locked,
                                                        value: slot.tool_id.clone().unwrap_or_default(),
                                                        onchange: {
                                                            let idx = *slot_index;
                                                            move |evt| {
                                                                let tool_id = evt.value();
                                                                let selected = if tool_id.trim().is_empty() { None } else { Some(tool_id) };
                                                                let result = state
                                                                    .with_mut(|s| {
                                                                        s.set_selected_toolset_slot_mode(idx, "fixed", selected)
                                                                    });
                                                                if let Err(message) = result {
                                                                    status_message.set(message);
                                                                }
                                                            }
                                                        },
                                                        option { value: "", "Select tool" }
                                                        for tool in snapshot.tools.iter() {
                                                            option { value: "{tool.id}",
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
                                state
                                    .with_mut(|s| {
                                        let result = s.clone_selected_toolset_profile();
                                        if result.is_ok() {
                                            let _ = s.rename_selected_toolset_profile(&name);
                                            s.log_event("Toolset profile cloned");
                                        }
                                        result
                                    })
                            } else {
                                state
                                    .with_mut(|s| {
                                        s.add_toolset_profile(&name);
                                        s.log_event("Toolset profile added");
                                        Ok(String::new())
                                    })
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
