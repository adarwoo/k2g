//! Job configuration sidebar — the shared right-hand panel beside every job view.
//! Selects the machining profile the live job runs, and the board orientation.
//! Outline-milling parameters (tabs, mouse bites, …) belong to the machining
//! profile's route step — they are edited in the Machining screen, not here.

use dioxus::prelude::*;
use units::Length;

use crate::runtime::AppCtx;
use crate::data::model::*;
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
            let side = if active_profile.side == Side::Bottom {
                "Bottom (solder side)".to_string()
            } else {
                "Top (component side)".to_string()
            };

            let mut rows: Vec<(&'static str, String)> = vec![
                ("Machining profile", active_profile.name.clone()),
                ("CNC", cnc_name),
                ("Fixture", fixture_name),
                ("Toolset", toolset_name),
                ("Side", side),
                ("Operations", operations),
                ("Cut depth", active_profile.cut_depth_strategy.label().to_string()),
            ];
            if active_profile.cut_depth_strategy == CutDepthStrategy::MultiPass {
                rows.push((
                    "Max depth/pass",
                    unit_format::format_length_display(active_profile.multi_pass_max_depth, snapshot.unit_system),
                ));
            }
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

                    if snapshot.selected_process_profile_id.is_none() {
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
