use dioxus::prelude::*;
use rfd::{MessageButtons, MessageDialog, MessageLevel};

use super::super::model::*;

#[component]
pub fn FixtureProfilesScreen(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let mut status_message = use_signal(String::new);

    let selected_fixture = snapshot.selected_fixture().cloned();

    rsx! {
        div { class: "screen split",
            section { class: "panel grow",
                div { class: "panel-header",
                    h3 { "Fixture profiles" }
                    div { class: "actions",
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                state.with_mut(|s| s.add_fixture_profile("Fixture profile"));
                                status_message.set("Fixture profile created".to_string());
                            },
                            "Add profile"
                        }
                        if let Some(fixture) = selected_fixture.as_ref() {
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
                                "Delete profile"
                            }
                        }
                    }
                }

                p {
                    "Fixture profiles define coordinate context and backing-board assumptions. They are selectable assets and can be referenced by job profiles."
                }

                if !status_message.read().is_empty() {
                    p { class: "diag-status", "{status_message}" }
                }

                if snapshot.fixtures.is_empty() {
                    p { class: "diag-status", "No fixture profiles available." }
                } else {
                    div { class: "profile-list",
                        for fixture in snapshot.fixtures.iter() {
                            div {
                                key: "{fixture.id}",
                                class: if snapshot.selected_fixture_id.as_ref() == Some(&fixture.id) { "profile-list-item active" } else { "profile-list-item" },
                                div {
                                    div { class: "profile-list-title", "{fixture.name}" }
                                    div { class: "profile-list-meta",
                                        "{fixture.coordinate_context} · {fixture.backing_board}"
                                    }
                                }
                                button {
                                    class: "btn btn-small",
                                    onclick: {
                                        let fixture_id = fixture.id.clone();
                                        move |_| {
                                            state.with_mut(|s| s.selected_fixture_id = Some(fixture_id.clone()));
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
