//! Job configuration sidebar — the shared right-hand panel beside every job view.
//! Selects the machining profile the live job runs, and the board orientation.
//! Outline-milling parameters (tabs, mouse bites, …) belong to the machining
//! profile's route step — they are edited in the Machining screen, not here.

use dioxus::prelude::*;
use units::Length;

use crate::runtime::AppCtx;
use units::user_format as unit_format;

/// The job-configuration sidebar. Reads the active job snapshot and writes edits
/// back through `mutate_ctx` (runtime job state).
#[component]
pub fn JobSidebar(state: Signal<AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let board_thickness_pcb_label = snapshot.board.as_ref().and_then(|board| board.thickness.as_ref()).map(
        |thickness| unit_format::format_length_display(Length::from_mm(thickness.as_mm()), snapshot.unit_system),
    );

    // The job summary as aligned (label, value) rows. Empty when no profile is
    // selected. A missing cnc/fixture/toolset renders as a broken-reference note.
    let summary_rows: Vec<(&'static str, String)> = snapshot
        .selected_process_profile()
        .map(|active_profile| {
            let cnc_name = snapshot
                .machines
                .iter()
                .find(|p| p.id == active_profile.cnc_profile_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("Broken reference ({})", active_profile.cnc_profile_id));
            let fixture_name = snapshot
                .fixtures
                .iter()
                .find(|p| p.id == active_profile.fixture_profile_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("Broken reference ({})", active_profile.fixture_profile_id));
            let toolset_name = snapshot
                .toolsets
                .iter()
                .find(|p| p.id == active_profile.toolset_profile_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("Broken reference ({})", active_profile.toolset_profile_id));
            let operations = {
                let ops = snapshot
                    .project_config
                    .selected_operations
                    .iter()
                    .map(|op| op.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                if ops.is_empty() { "—".to_string() } else { ops }
            };
            let board_face = active_profile.board_face.label().to_string();

            let mut rows: Vec<(&'static str, String)> = vec![
                ("Machining profile", active_profile.name.clone()),
                ("CNC", cnc_name),
                ("Fixture", fixture_name),
                ("Toolset", toolset_name),
                ("Board face", board_face),
                ("Operations", operations),
            ];
            rows.push((
                "Board thickness",
                board_thickness_pcb_label.clone().unwrap_or_else(|| "—".to_string()),
            ));
            rows
        })
        .unwrap_or_default();

    rsx! {
                section { class: "panel fixed",
                    h3 { "Job configuration" }

                    div { class: "field",
                        label { "Machining profile" }
                        select {
                            value: snapshot.selected_process_profile_id.clone().unwrap_or_default(),
                            onchange: move |evt| {
                                let value = evt.value();
                                crate::ui::screens::mutate_ctx(
                                    state,
                                    |s| {
                                        let selected = if value.trim().is_empty() { None } else { Some(value) };
                                        s.select_process_profile_by_id(selected);
                                    },
                                );
                            },
                            option {
                                value: "",
                                selected: snapshot.selected_process_profile_id.is_none(),
                                "Select machining profile"
                            }
                            for profile in snapshot.process_profiles.iter() {
                                option {
                                    value: "{profile.id}",
                                    selected: snapshot.selected_process_profile_id.as_deref() == Some(profile.id.as_str()),
                                    "{profile.name}"
                                }
                            }
                        }
                        p { class: "diag-status",
                            "The job runs this machining profile — its ordered machining steps."
                        }
                    }

                    // Nothing to select from at all. This is a fresh install, and it is the
                    // point the manual's quick start turns into ten minutes of authoring —
                    // so the offer to build the set is made here, where the wall is, rather
                    // than on a screen the newcomer has not thought to open.
                    if snapshot.process_profiles.is_empty() {
                        StarterKitOffer { state }
                    } else if snapshot.selected_process_profile_id.is_none() {
                        p { class: "diag-status",
                            "Select a machining profile to display job attributes."
                        }
                    }

                    if snapshot.selected_process_profile_id.is_some() {
                        if !summary_rows.is_empty() {
                            div { class: "field",
                                label { "Job summary" }
                                div { class: "job-summary",
                                    for (name , value) in summary_rows.iter() {
                                        span { class: "job-summary-label", "{name}" }
                                        span { class: "job-summary-value", "{value}" }
                                    }
                                }
                            }
                        }

                        div { class: "field",
                            label { "Board orientation angle" }
                            p { class: "diag-status", "Angle in degrees. 0 is default." }
                            input {
                                r#type: "number",
                                min: "-180",
                                max: "180",
                                step: "1",
                                value: "{snapshot.project_config.rotation_angle}",
                                oninput: move |evt| {
                                    let value = evt.value().parse::<i32>().unwrap_or(0);
                                    crate::ui::screens::mutate_ctx(state, |s| s.set_board_orientation(value));
                                },
                            }
                        }
                    }
                }
    }
}

/// The first-run offer: build a complete, working set of profiles from one choice.
///
/// Shown only when there is no machining profile at all, which is a fresh install and
/// nothing else. The alternative for that user is the manual's quick start — five objects
/// authored before anything can be generated, of which the machine is the only one they
/// are equipped to decide. So the machine is the only thing asked for.
///
/// Deliberately not a silent "set everything up" button. It says what it is about to
/// create and what has to be checked afterwards, because one of the values it writes is
/// the backboard thickness that keeps a drill out of the machine bed — and a starter set
/// that hides that is worse than the wall it replaces.
#[component]
fn StarterKitOffer(state: Signal<AppCtx>) -> Element {
    let templates = crate::ui::bindings::use_templates(crate::data::Profile::Cnc);
    let mut chosen = use_signal(|| {
        templates.first().map(|(key, _)| key.clone()).unwrap_or_default()
    });
    let mut message = use_signal(String::new);

    rsx! {
        div { class: "starter-kit",
            p { class: "starter-kit-lead",
                "No machining profile yet. Pick your machine and k2g will create a matching "
                "fixture, toolset and machining profile, ready to generate."
            }

            div { class: "field",
                label { "Machine" }
                select {
                    value: "{chosen}",
                    onchange: move |evt| chosen.set(evt.value()),
                    for (key , label) in templates.iter() {
                        option { value: "{key}", "{label}" }
                    }
                }
            }

            button {
                class: "btn btn-primary",
                r#type: "button",
                disabled: chosen.read().is_empty(),
                onclick: move |_| {
                    let key = chosen.read().clone();
                    match crate::ui::bindings::create_starter_kit(&key) {
                        Some(machining) => {
                            let id = machining.to_string();
                            crate::ui::screens::mutate_ctx(
                                state,
                                move |s| s.select_process_profile_by_id(Some(id.clone())),
                            );
                            message.set(String::new());
                        }
                        None => message.set(
                            "Could not create the starter profiles — see Logs for detail."
                                .to_string(),
                        ),
                    }
                },
                "Create starter profiles"
            }

            p { class: "diag-status starter-kit-check",
                "Then check the fixture: its backboard thickness is what keeps the drill out "
                "of your machine bed, and it ships at a conservative value rather than a "
                "measurement of your bench."
            }

            if !message.read().is_empty() {
                p { class: "diag-status", "{message}" }
            }
        }
    }
}
