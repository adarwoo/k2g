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
pub fn FixtureProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);
    let mut profile_filter = use_signal(String::new);
    let mut add_action = use_signal(|| ADD_ACTION_NEW.to_string());

    let selected_fixture = snapshot.selected_fixture().cloned();
    let profile_filter_value = profile_filter.read().clone();
    let filtered_fixtures = snapshot
        .fixtures
        .iter()
        .filter(|fixture| {
            profile_matches_filter(&fixture.name, &fixture.id, &profile_filter_value)
        })
        .cloned()
        .collect::<Vec<_>>();

    rsx! {
        div { class: "screen single",
            section { class: "panel grow",
                article { class: "setup-card section-block cnc-manager-shell",
                    div { class: "panel-header",
                        div {
                            h3 { "Fixture profile management" }
                            p {
                                "Fixture profiles describe holding/origin assumptions and are referenced by processing profiles."
                            }
                        }
                        ProfileAddDropdown {
                            selected_action: add_action.read().clone(),
                            include_template: false,
                            on_action_change: move |value| add_action.set(value),
                            on_add: move |_| {
                                match add_action.read().as_str() {
                                    ADD_ACTION_NEW => {
                                        state.with_mut(|s| s.add_fixture_profile("Fixture profile"));
                                        status_message.set("Fixture profile created".to_string());
                                    }
                                    ADD_ACTION_TEMPLATE => {
                                        status_message
                                            .set("No fixture template is available".to_string());
                                    }
                                    ADD_ACTION_EXISTING => {
                                        let result = state.with_mut(|s| s.clone_selected_fixture_profile());
                                        match result {
                                            Ok(_) => status_message.set("Fixture profile cloned".to_string()),
                                            Err(message) => status_message.set(message),
                                        }
                                    }
                                    ADD_ACTION_EXTERNAL => {
                                        let picked = FileDialog::new()
                                            .set_title("Import fixture profile")
                                            .add_filter("Fixture profile YAML", &["yaml", "yml"])
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
                                                    .set(
                                                        "Fixture profile import failed: file not readable"
                                                            .to_string(),
                                                    );
                                                return;
                                            }
                                        };
                                        let result = state.with_mut(|s| s.import_fixture_profile_yaml(&text));
                                        match result {
                                            Ok(_) => {
                                                status_message
                                                    .set("Fixture profile imported and selected".to_string())
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

                            if filtered_fixtures.is_empty() {
                                p { class: "diag-status", "No matching profiles found." }
                            } else {
                                div { class: "profile-list",
                                    for fixture in filtered_fixtures.into_iter() {
                                        div {
                                            key: "{fixture.id}",
                                            class: if snapshot.selected_fixture_id.as_ref() == Some(&fixture.id) { "profile-list-item active editable" } else { "profile-list-item editable" },
                                            onclick: {
                                                let fixture_id = fixture.id.clone();
                                                move |_| {
                                                    state.with_mut(|s| s.selected_fixture_id = Some(fixture_id.clone()));
                                                }
                                            },
                                            div {
                                                div { class: "profile-list-title", "{fixture.name}" }
                                                div { class: "profile-list-meta",
                                                    "{fixture.coordinate_context} · {fixture.backing_board}"
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
                            if let Some(fixture) = selected_fixture.as_ref() {
                                div { class: "panel-header",
                                    div {
                                        h4 { "{fixture.name}" }
                                        p { "Editable profile" }
                                    }
                                    div { class: "actions",
                                        button {
                                            class: "btn btn-secondary",
                                            onclick: {
                                                let fixture = fixture.clone();
                                                move |_| {
                                                    let default_name = format!(
                                                        "{}.fixture-profile.yaml",
                                                        slug_file_name(&fixture.name, "fixture-profile"),
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
                                                        status_message.set("Fixture profile exported".to_string());
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
                                                let fixture_id = fixture.id.clone();
                                                move |_| {
                                                    let impact = state.read().impact_delete_fixture_profile(&fixture_id);
                                                    let description = format_impact_warning(
                                                        "Delete fixture profile and dependent assets?",
                                                        &impact,
                                                    );
                                                    let confirmed = MessageDialog::new()
                                                        .set_level(MessageLevel::Warning)
                                                        .set_title("Delete fixture profile")
                                                        .set_description(&description)
                                                        .set_buttons(MessageButtons::YesNo)
                                                        .show();
                                                    if confirmed == rfd::MessageDialogResult::Yes {
                                                        let impact = state
                                                            .with_mut(|s| s.delete_fixture_profile_with_cascade(&fixture_id));
                                                        status_message
                                                            .set(format_impact_summary("Deleted fixture profile", &impact));
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
                        }
                    }
                }

                if selected_fixture.is_none() {
                    p { class: "diag-status", "Select a fixture profile to edit details." }
                }
            }
        }
    }
}


