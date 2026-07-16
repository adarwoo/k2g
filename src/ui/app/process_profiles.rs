use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use serde_json::Value;
use std::fs;

use super::super::model::*;
use super::profiles_common::{
    format_impact_warning, slug_file_name, BindingListSelector, ProfileLifecycleToolbar,
    ProfileNameDialog,
};

#[component]
pub fn MachiningProfilesScreen(state: Signal<crate::app_state_impl::AppCtx>) -> Element {
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
                        super::mutate_ctx(
                            state,
                            |s| {
                                s.select_process_profile_by_id(Some(id.clone()));
                                s.mark_last_edited_process_profile(Some(id));
                            },
                        );
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
                            super::mutate_ctx(
                                state,
                                |s| {
                                    let _ = s.delete_process_profile_with_cascade(&profile_id);
                                    s.log_event("Machining profile deleted");
                                },
                            );
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
                        div { class: "edit-grid process-edit-grid",
                            div { class: if profile.pending_required_fields.contains("cnc.default")
    || profile.pending_required_fields.contains("cnc.choices") { "field required-pending" } else { "field" },
                                BindingListSelector {
                                    label: "CNC profile".to_string(),
                                    options: snapshot
                                        .machines
                                        .iter()
                                        .map(|machine| {
                                            let suffix = if machine.usable { "" } else { " (not usable)" };
                                            (machine.id.clone(), format!("{}{}", machine.name, suffix))
                                        })
                                        .collect::<Vec<_>>(),
                                    selected_ids: profile.cnc_profile_choices.clone(),
                                    default_id: profile.cnc_profile_id.clone(),
                                    on_change: move |(selected_ids, default_id): (Vec<String>, String)| {
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| s.set_selected_process_profile_cnc_binding(&default_id, &selected_ids),
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("fixture.default")
    || profile.pending_required_fields.contains("fixture.choices") { "field required-pending" } else { "field" },
                                BindingListSelector {
                                    label: "Fixture profile".to_string(),
                                    options: snapshot
                                        .fixtures
                                        .iter()
                                        .map(|fixture| {
                                            let suffix = if fixture.usable { "" } else { " (not usable)" };
                                            (fixture.id.clone(), format!("{}{}", fixture.name, suffix))
                                        })
                                        .collect::<Vec<_>>(),
                                    selected_ids: profile.fixture_profile_choices.clone(),
                                    default_id: profile.fixture_profile_id.clone(),
                                    on_change: move |(selected_ids, default_id): (Vec<String>, String)| {
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| {
                                                s
                                                    .set_selected_process_profile_fixture_binding(
                                                    &default_id,
                                                    &selected_ids,
                                                )
                                            },
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("toolset.default")
    || profile.pending_required_fields.contains("toolset.choices") { "field required-pending" } else { "field" },
                                BindingListSelector {
                                    label: "Toolset profile".to_string(),
                                    options: snapshot
                                        .toolsets
                                        .iter()
                                        .map(|toolset| {
                                            let suffix = if toolset.usable { "" } else { " (not usable)" };
                                            (toolset.id.clone(), format!("{}{}", toolset.name, suffix))
                                        })
                                        .collect::<Vec<_>>(),
                                    selected_ids: profile.toolset_profile_choices.clone(),
                                    default_id: profile.toolset_profile_id.clone(),
                                    on_change: move |(selected_ids, default_id): (Vec<String>, String)| {
                                        let result = super::mutate_ctx(
                                            state,
                                            |s| {
                                                s
                                                    .set_selected_process_profile_toolset_binding(
                                                    &default_id,
                                                    &selected_ids,
                                                )
                                            },
                                        );
                                        if let Err(message) = result {
                                            status_message.set(message);
                                        }
                                    },
                                }
                            }

                            div { class: if profile.pending_required_fields.contains("operations") { "field required-pending" } else { "field" },
                                label { "Operations" }
                                p { class: "hint",
                                    "Select enabled operations. Configuration sections appear below."
                                }

                                div { class: "operation-group",
                                    p { class: "hint", "Drilling" }
                                    for (idx , op) in [
                                        ProductionOperation::DrillLocatingPins,
                                        ProductionOperation::DrillPth,
                                        ProductionOperation::DrillNpth,
                                    ]
                                        .iter()
                                        .enumerate()
                                    {
                                        label {
                                            key: "drill-op-{idx}",
                                            class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: profile.default_operations.contains(op),
                                                oninput: {
                                                    let operation = *op;
                                                    move |_| {
                                                        let result = super::mutate_ctx(
                                                            state,
                                                            |s| s.toggle_selected_process_profile_operation(operation),
                                                        );
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

                                div { class: "operation-group",
                                    p { class: "hint", "Board Edge" }
                                    for (idx , op) in [ProductionOperation::RouteBoard, ProductionOperation::MillBoard].iter().enumerate() {
                                        label {
                                            key: "edge-op-{idx}",
                                            class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: profile.default_operations.contains(op),
                                                oninput: {
                                                    let operation = *op;
                                                    move |_| {
                                                        let result = super::mutate_ctx(
                                                            state,
                                                            |s| s.toggle_selected_process_profile_operation(operation),
                                                        );
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

                            for op in selected_operations_in_order(profile) {
                                div { class: "field operation-config-section",
                                    h4 { "{op.label()}" }
                                    hr {}

                                    if op == ProductionOperation::DrillLocatingPins {
                                        p { class: "hint", "No extra options for locating pins yet." }
                                    }

                                    if op == ProductionOperation::DrillPth || op == ProductionOperation::DrillNpth {
                                        div { class: "field",
                                            label { "Oversize allowance (relative)" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["holes", "oversize", "relative"], "8%"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["holes", "oversize", "relative"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        div { class: "field",
                                            label { "Oversize allowance (max)" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["holes", "oversize", "max"], "0.20mm"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["holes", "oversize", "max"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        div { class: "field",
                                            label { "Undersize allowance (relative)" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["holes", "undersize", "relative"], "8%"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["holes", "undersize", "relative"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        div { class: "field",
                                            label { "Undersize allowance (max)" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["holes", "undersize", "max"], "0.20mm"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["holes", "undersize", "max"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        label { class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: operation_bool(profile, op, &["holes", "route_fallback"], false),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_bool(
                                                                op,
                                                                &["holes", "route_fallback"],
                                                                evt.value() == "true",
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                            span { "Route fallback for unsupported holes" }
                                        }

                                        label { class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: operation_bool(profile, op, &["holes", "drill_first"], true),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_bool(
                                                                op,
                                                                &["holes", "drill_first"],
                                                                evt.value() == "true",
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                            span { "Drill before contour operations" }
                                        }

                                        label { class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: operation_bool(profile, op, &["holes", "pilot"], false),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_bool(
                                                                op,
                                                                &["holes", "pilot"],
                                                                evt.value() == "true",
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                            span { "Enable pilot drilling" }
                                        }

                                        div { class: "field",
                                            label { "Oblong hole strategy" }
                                            select {
                                                value: operation_string(profile, op, &["holes", "oblong"], "drill_ends_then_route"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["holes", "oblong"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                                option { value: "route", "Route" }
                                                option { value: "drill_ends_then_route",
                                                    "Drill ends then route"
                                                }
                                                option { value: "drill_chain", "Drill chain" }
                                                option { value: "drill_chain_then_route",
                                                    "Drill chain then route"
                                                }
                                            }
                                        }
                                    }

                                    if op == ProductionOperation::RouteBoard {
                                        div { class: "field",
                                            label { "Edge cut mode" }
                                            select {
                                                value: operation_string(profile, op, &["edge", "cut"], "route"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["edge", "cut"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                                option { value: "route", "Route" }
                                                option { value: "mill", "Mill" }
                                                option { value: "score", "Score" }
                                                option { value: "vgroove", "V-groove" }
                                            }
                                        }

                                        div { class: "field",
                                            label { "Edge retention" }
                                            select {
                                                value: operation_string(profile, op, &["edge", "retention"], "tabs"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["edge", "retention"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                                option { value: "none", "None" }
                                                option { value: "tabs", "Tabs" }
                                                option { value: "mouse_bites", "Mouse bites" }
                                                option { value: "tabs_with_mouse_bites",
                                                    "Tabs with mouse bites"
                                                }
                                            }
                                        }

                                        div { class: "field",
                                            label { "Tab count" }
                                            input {
                                                r#type: "number",
                                                min: "0",
                                                value: operation_u64(profile, op, &["edge", "tabs"], 4).to_string(),
                                                oninput: move |evt| {
                                                    let parsed = evt.value().parse::<u64>().unwrap_or(0);
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| { s.set_selected_process_operation_u64(op, &["edge", "tabs"], parsed) },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        div { class: "field",
                                            label { "Tab width" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["edge", "tab_width"], "2.0mm"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["edge", "tab_width"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        div { class: "field",
                                            label { "Mouse bite hole count" }
                                            input {
                                                r#type: "number",
                                                min: "1",
                                                value: operation_u64(profile, op, &["edge", "bite_holes"], 3).to_string(),
                                                oninput: move |evt| {
                                                    let parsed = evt.value().parse::<u64>().unwrap_or(1);
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_u64(op, &["edge", "bite_holes"], parsed)
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }

                                        if operation_string(profile, op, &["edge", "cut"], "route") == "vgroove" {
                                            div { class: "field",
                                                label { "V-groove depth" }
                                                input {
                                                    r#type: "text",
                                                    value: operation_string(profile, op, &["edge", "vgroove_depth"], "80%"),
                                                    oninput: move |evt| {
                                                        let result = super::mutate_ctx(
                                                            state,
                                                            |s| {
                                                                s.set_selected_process_operation_string(
                                                                    op,
                                                                    &["edge", "vgroove_depth"],
                                                                    evt.value(),
                                                                )
                                                            },
                                                        );
                                                        if let Err(message) = result {
                                                            status_message.set(message);
                                                        }
                                                    },
                                                }
                                            }
                                        }
                                    }

                                    if op == ProductionOperation::RouteBoard || op == ProductionOperation::MillBoard {
                                        div { class: "field",
                                            label { "Finish direction" }
                                            select {
                                                value: operation_string(profile, op, &["finishing", "direction"], "climb"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["finishing", "direction"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                                option { value: "conventional", "Conventional" }
                                                option { value: "climb", "Climb" }
                                            }
                                        }

                                        div { class: "field",
                                            label { "Finish clearance" }
                                            input {
                                                r#type: "text",
                                                value: operation_string(profile, op, &["finishing", "clearance"], "0.1mm"),
                                                oninput: move |evt| {
                                                    let result = super::mutate_ctx(
                                                        state,
                                                        |s| {
                                                            s.set_selected_process_operation_string(
                                                                op,
                                                                &["finishing", "clearance"],
                                                                evt.value(),
                                                            )
                                                        },
                                                    );
                                                    if let Err(message) = result {
                                                        status_message.set(message);
                                                    }
                                                },
                                            }
                                        }
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
                            super::mutate_ctx(
                                state,
                                |s| {
                                    let result = s.clone_selected_process_profile();
                                    if result.is_ok() {
                                        let _ = s.rename_selected_process_profile(&name);
                                        s.log_event("Machining profile cloned");
                                    }
                                    result
                                },
                            )
                        } else {
                            super::mutate_ctx(
                                state,
                                |s| {
                                    s.add_process_profile(&name);
                                    s.log_event("Machining profile added");
                                    Ok(String::new())
                                },
                            )
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

fn selected_operations_in_order(profile: &JobProfile) -> Vec<ProductionOperation> {
    ProductionOperation::all()
        .into_iter()
        .filter(|op| profile.default_operations.contains(op))
        .collect()
}

fn operation_value<'a>(profile: &'a JobProfile, op: ProductionOperation, path: &[&str]) -> Option<&'a Value> {
    let mut current = profile.operation_setups.get(operation_key(op))?;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn operation_bool(profile: &JobProfile, op: ProductionOperation, path: &[&str], fallback: bool) -> bool {
    operation_value(profile, op, path)
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

fn operation_string(profile: &JobProfile, op: ProductionOperation, path: &[&str], fallback: &str) -> String {
    operation_value(profile, op, path)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

fn operation_u64(profile: &JobProfile, op: ProductionOperation, path: &[&str], fallback: u64) -> u64 {
    operation_value(profile, op, path)
        .and_then(Value::as_u64)
        .unwrap_or(fallback)
}

fn operation_key(op: ProductionOperation) -> &'static str {
    match op {
        ProductionOperation::DrillLocatingPins => "drill_locating_pins",
        ProductionOperation::DrillPth => "drill_pth",
        ProductionOperation::DrillNpth => "drill_npth",
        ProductionOperation::RouteBoard => "route_board",
        ProductionOperation::MillBoard => "mill_board",
    }
}
