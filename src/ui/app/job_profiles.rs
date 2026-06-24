use dioxus::prelude::*;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use super::super::model::*;

#[component]
pub fn JobProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);

    let selected_job_profile = snapshot.selected_job_profile().cloned();
    let profile_rows: Vec<(JobProfile, String, String, bool)> = snapshot
        .job_profiles
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
            let is_active = snapshot.selected_job_profile_id.as_ref() == Some(&profile.id);
            (profile, cnc_name, fixture_name, is_active)
        })
        .collect();

    rsx! {
        div { class: "screen split",
            section { class: "panel grow",
                div { class: "panel-header",
                    h3 { "Job profiles" }
                    div { class: "actions",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                state.with_mut(|s| s.add_job_profile("Job profile"));
                                status_message.set("Job profile created".to_string());
                            },
                            "Add profile"
                        }
                        if let Some(profile) = selected_job_profile.as_ref() {
                            button {
                                class: "btn btn-danger",
                                onclick: {
                                    let profile_id = profile.id.clone();
                                    move |_| {
                                        let impact = state.read().impact_delete_job_profile(&profile_id);
                                        let description = format_impact_warning(
                                            "Delete job profile and dependent assets?",
                                            &impact,
                                        );
                                        let confirmed = MessageDialog::new()
                                            .set_level(MessageLevel::Warning)
                                            .set_title("Delete job profile")
                                            .set_description(&description)
                                            .set_buttons(MessageButtons::YesNo)
                                            .show();
                                        if confirmed == rfd::MessageDialogResult::Yes {
                                            let impact = state
                                                .with_mut(|s| s.delete_job_profile_with_cascade(&profile_id));
                                            status_message
                                                .set(format_impact_summary("Deleted job profile", &impact));
                                        }
                                    }
                                },
                                "Delete profile"
                            }
                        }
                    }
                }

                p {
                    "Job profiles predefine machining selections and bind one CNC profile and one fixture profile. Live jobs are instantiated from a selected job profile."
                }

                if !status_message.read().is_empty() {
                    p { class: "diag-status", "{status_message}" }
                }

                if snapshot.job_profiles.is_empty() {
                    p { class: "diag-status", "No job profiles available." }
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
                                            state.with_mut(|s| s.selected_job_profile_id = Some(profile_id.clone()));
                                        }
                                    },
                                    "Select"
                                }
                            }
                        }
                    }
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
    for item in impact.dependent_job_profiles.iter() {
        lines.push(format!("- {}", item));
    }
    for item in impact.deleted_live_jobs.iter() {
        lines.push(format!("- {}", item));
    }
    lines.join("\n")
}

fn format_impact_summary(prefix: &str, impact: &CascadeDeleteImpact) -> String {
    format!(
        "{}: {} primary, {} dependent job profile(s), {} live job(s)",
        prefix,
        impact.primary_profiles.len(),
        impact.dependent_job_profiles.len(),
        impact.deleted_live_jobs.len()
    )
}
