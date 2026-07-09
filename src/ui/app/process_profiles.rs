use dioxus::prelude::*;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::fs;

use super::super::model::*;
use super::profiles_common::{
    format_impact_summary, format_impact_warning, profile_matches_filter, slug_file_name,
    ProfileAddDropdown, ADD_ACTION_EXISTING, ADD_ACTION_EXTERNAL, ADD_ACTION_NEW,
    ADD_ACTION_TEMPLATE,
};

#[component]
pub fn ProcessProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut profile_filter = use_signal(String::new);
    let mut add_action = use_signal(|| ADD_ACTION_NEW.to_string());

    let selected_process_profile = snapshot.selected_process_profile().cloned();
    let profile_filter_value = profile_filter.read().clone();
    let profile_rows: Vec<(JobProfile, String, String, String, bool)> = snapshot
        .process_profiles
        .iter()
        .cloned()
        .map(|profile| {
            let cnc_name = snapshot
                .machines
                .iter()
                .find(|machine| machine.id == profile.cnc_profile_id)
                .map(|machine| machine.name.clone())
                .unwrap_or_else(|| "Missing CNC".to_string());
            let fixture_name = snapshot
                .fixtures
                .iter()
                .find(|fixture| fixture.id == profile.fixture_profile_id)
                .map(|fixture| fixture.name.clone())
                .unwrap_or_else(|| "Missing Fixture".to_string());
            let toolset_name = snapshot
                .toolsets
                .iter()
                .find(|toolset| toolset.id == profile.toolset_profile_id)
                .map(|toolset| toolset.name.clone())
                .unwrap_or_else(|| "Missing Toolset".to_string());
            let is_active = snapshot.selected_process_profile_id.as_ref() == Some(&profile.id);
            (profile, cnc_name, fixture_name, toolset_name, is_active)
        })
        .filter(|(profile, _, _, _, _)| {
            profile_matches_filter(&profile.name, &profile.id, &profile_filter_value)
        })
        .collect();

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
                        ProfileAddDropdown {
                            selected_action: add_action.read().clone(),
                            include_template: false,
                            on_action_change: move |value| add_action.set(value),
                            on_add: move |_| {
                                match add_action.read().as_str() {
                                    ADD_ACTION_NEW => {
                                        state.with_mut(|s| s.add_process_profile("Processing profile"));
                                        status_message.set("Processing profile created".to_string());
                                    }
                                    ADD_ACTION_TEMPLATE => {
                                        status_message
                                            .set("No processing template is available".to_string());
                                    }
                                    ADD_ACTION_EXISTING => {
                                        let result = state.with_mut(|s| s.clone_selected_process_profile());
                                        match result {
                                            Ok(_) => status_message.set("Processing profile cloned".to_string()),
                                            Err(message) => status_message.set(message),
                                        }
                                    }
                                    ADD_ACTION_EXTERNAL => {
                                        let picked = FileDialog::new()
                                            .set_title("Import processing profile")
                                            .add_filter("Processing profile YAML", &["yaml", "yml"])
                                            .pick_file();

                                        let Some(path) = picked else {
                                            status_message.set("Import canceled".to_string());
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
                                                status_message
                                                    .set("Processing profile imported and selected".to_string())
                                            }
                                            Err(message) => status_message.set(message),
                                        }
                                    }
                                    _ => {}
                                }
                            },
                        }
                    }

                    div { class: "cnc-manager-grid",
                        div { class: "setup-card cnc-profile-list-panel",
                            h4 { "Profiles" }
                            input {
                                class: "stock-filter-input",
                                value: profile_filter_value,
                                placeholder: "Search profiles",
                                oninput: move |evt| profile_filter.set(evt.value()),
                            }

                            if !status_message.read().is_empty() {
                                p { class: "diag-status", "{status_message}" }
                            }

                            if profile_rows.is_empty() {
                                p { class: "diag-status", "No matching profiles found." }
                            } else {
                                div { class: "profile-list",
                                    for (profile , cnc_name , fixture_name , toolset_name , is_active) in profile_rows.into_iter() {
                                        div {
                                            key: "{profile.id}",
                                            class: if is_active { "profile-list-item active editable" } else { "profile-list-item editable" },
                                            onclick: {
                                                let profile_id = profile.id.clone();
                                                move |_| {
                                                    state.with_mut(|s| s.select_process_profile_by_id(Some(profile_id.clone())));
                                                }
                                            },
                                            div {
                                                div { class: "profile-list-title", "{profile.name}" }
                                                div { class: "profile-list-meta",
                                                    "CNC: {cnc_name} · Fixture: {fixture_name} · Toolset: {toolset_name}"
                                                }
                                            }
                                            span { class: "status-chip status-in-stock",
                                                "My profile"
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        div { class: "setup-card cnc-profile-details-panel",
                            if let Some(profile) = selected_process_profile.as_ref() {
                                div { class: "panel-header",
                                    div {
                                        h4 { "{profile.name}" }
                                        p { "Editable profile" }
                                    }
                                    div { class: "actions",
                                        button {
                                            class: "btn btn-secondary",
                                            onclick: {
                                                let profile = profile.clone();
                                                move |_| {
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
                                                        status_message.set("Processing profile exported".to_string());
                                                    } else {
                                                        status_message.set("Export failed: unable to write file".to_string());
                                                    }
                                                }
                                            },
                                            "Export"
                                        }
                                        button {
                                            class: "btn btn-danger",
                                            onclick: {
                                                let profile_id = profile.id.clone();
                                                move |_| {
                                                    let impact = state.read().impact_delete_process_profile(&profile_id);
                                                    let description = format_impact_warning(
                                                        "Delete processing profile and dependent assets?",
                                                        &impact,
                                                    );
                                                    let confirmed = MessageDialog::new()
                                                        .set_level(MessageLevel::Warning)
                                                        .set_title("Delete processing profile")
                                                        .set_description(&description)
                                                        .set_buttons(MessageButtons::YesNo)
                                                        .show();
                                                    if confirmed == rfd::MessageDialogResult::Yes {
                                                        let impact = state
                                                            .with_mut(|s| s.delete_process_profile_with_cascade(&profile_id));
                                                        status_message
                                                            .set(format_impact_summary("Deleted processing profile", &impact));
                                                    }
                                                }
                                            },
                                            "Delete"
                                        }
                                    }
                                }

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
                                p { class: "diag-status",
                                    "Select a processing profile to edit details."
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
