use dioxus::prelude::*;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use super::super::model::*;

#[component]
pub fn ProcessProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);

    let selected_process_profile = snapshot.selected_process_profile().cloned();
    let profile_rows: Vec<(JobProfile, String, String, bool)> = snapshot
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
            let is_active = snapshot.selected_process_profile_id.as_ref() == Some(&profile.id);
            (profile, cnc_name, fixture_name, is_active)
        })
        .collect();

    rsx! {
        div { class: "screen split",
            section { class: "panel grow",
                div { class: "panel-header",
                    h3 { "Processing profiles" }
                    div { class: "actions",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                state.with_mut(|s| s.add_process_profile("Processing profile"));
                                status_message.set("Processing profile created".to_string());
                            },
                            "Add profile"
                        }
                        button {
                            class: "btn btn-secondary",
                            disabled: selected_process_profile.is_none(),
                            onclick: move |_| {
                                let result = state.with_mut(|s| s.clone_selected_process_profile());
                                match result {
                                    Ok(_) => status_message.set("Processing profile cloned".to_string()),
                                    Err(message) => status_message.set(message),
                                }
                            },
                            "Clone profile"
                        }
                        if let Some(profile) = selected_process_profile.as_ref() {
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
                                "Delete profile"
                            }
                        }
                    }
                }

                p {
                    "Processing profiles define default operations and bind one CNC profile and one fixture profile. There are no built-in processing profiles."
                }

                if !status_message.read().is_empty() {
                    p { class: "diag-status", "{status_message}" }
                }

                if snapshot.process_profiles.is_empty() {
                    p { class: "diag-status", "No processing profiles available." }
                } else {
                    div { class: "profile-list",
                        for (profile , cnc_name , fixture_name , is_active) in profile_rows.into_iter() {
                            div {
                                key: "{profile.id}",
                                class: if is_active { "profile-list-item active" } else { "profile-list-item" },
                                div {
                                    div { class: "profile-list-title", "{profile.name}" }
                                    div { class: "profile-list-meta",
                                        "CNC: {cnc_name} · Fixture: {fixture_name}"
                                    }
                                }
                                button {
                                    class: "btn btn-small",
                                    onclick: {
                                        let profile_id = profile.id.clone();
                                        move |_| {
                                            state.with_mut(|s| s.select_process_profile_by_id(Some(profile_id.clone())));
                                        }
                                    },
                                    "Select"
                                }
                            }
                        }
                    }
                }
            }

            section { class: "panel fixed",
                h3 { "Profile details" }

                if let Some(profile) = selected_process_profile.as_ref() {
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
                } else {
                    p { class: "diag-status", "Select a processing profile to edit details." }
                }
            }
        }
    }
}

fn format_impact_warning(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    let mut lines = vec![prefix.to_string()];
    for item in impact.primary_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.dependent_process_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.deleted_live_projects.iter() {
        lines.push(format!("- {}", item));
    }
    lines.join("\n")
}

fn format_impact_summary(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    format!(
        "{}: {} primary, {} dependent process profile(s), {} live project(s)",
        prefix,
        impact.primary_profiles.len(),
        impact.dependent_process_profiles.len(),
        impact.deleted_live_projects.len()
    )
}


