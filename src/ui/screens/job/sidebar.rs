//! Job configuration sidebar — the shared right-hand panel beside every job view.
//! Selects the machining profile and, when board-outline milling is enabled, the
//! outline router / mouse-bite drill tools and their tab-width / pitch parameters.
//! Edits apply to the runtime job config (the current job is not persisted).

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
    let milling_outline_enabled = snapshot
        .project_config
        .selected_operations
        .iter()
        .any(|op| matches!(op, ProductionOperation::RouteBoard | ProductionOperation::MillBoard));
    let tab_width_display = unit_format::format_length_input_value_from_mm(
        snapshot.project_config.tab_width.as_mm(),
        snapshot.unit_system,
    );
    let tab_width_is_overridden = (snapshot.project_config.tab_width.as_mm()
        - snapshot.project_config.tab_width_baseline.as_mm())
    .abs()
        > 1e-6;
    let tab_width_step = unit_format::length_input_step(snapshot.unit_system);
    let tab_width_display_label = unit_format::format_length_display(
        snapshot.project_config.tab_width,
        snapshot.unit_system,
    );
    let mouse_bite_pitch_display_label = unit_format::format_length_display(
        snapshot.project_config.mouse_bite_pitch,
        snapshot.unit_system,
    );
    let tab_width_hint = match snapshot.unit_system {
        UserUnitSystem::Metric => "2.4mm",
        UserUnitSystem::Imperial => "1/16in",
        UserUnitSystem::Mil => "95mil",
    };
    let mouse_bite_pitch_display = unit_format::format_length_input_value_from_mm(
        snapshot.project_config.mouse_bite_pitch.as_mm(),
        snapshot.unit_system,
    );
    let mouse_bite_pitch_min = match snapshot.unit_system {
        UserUnitSystem::Metric => "0.6",
        UserUnitSystem::Imperial => "0.024",
        UserUnitSystem::Mil => "24",
    };
    let mouse_bite_pitch_max = match snapshot.unit_system {
        UserUnitSystem::Metric => "1.5",
        UserUnitSystem::Imperial => "0.059",
        UserUnitSystem::Mil => "59",
    };
    let eligible_router_tools: Vec<&Tool> = snapshot
        .tools
        .iter()
        .filter(|tool| {
            tool.kind.to_ascii_lowercase().contains("router")
                && {
                    let d = tool.diameter.as_mm();
                    (0.8..=2.5).contains(&d)
                }
        })
        .collect();
    let selected_router_diameter_mm = snapshot
        .project_config
        .outline_router_tool_id
        .as_ref()
        .and_then(|id| {
            eligible_router_tools
                .iter()
                .find(|tool| tool.id == *id)
                .map(|tool| tool.diameter.as_mm())
        });
    let eligible_mouse_bite_drills: Vec<&Tool> = snapshot
        .tools
        .iter()
        .filter(|tool| {
            if !tool.kind.to_ascii_lowercase().contains("drill") {
                return false;
            }
            let d = tool.diameter.as_mm();
            if !(0.5..=1.5).contains(&d) {
                return false;
            }
            if let Some(router_d) = selected_router_diameter_mm {
                d <= router_d
            } else {
                false
            }
        })
        .collect();
    let router_ref_is_broken = snapshot
        .project_config
        .outline_router_tool_id
        .as_ref()
        .map(|id| !eligible_router_tools.iter().any(|tool| tool.id == *id))
        .unwrap_or(false);
    let drill_ref_is_broken = snapshot
        .project_config
        .mouse_bite_drill_tool_id
        .as_ref()
        .map(|id| !eligible_mouse_bite_drills.iter().any(|tool| tool.id == *id))
        .unwrap_or(false);

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
                            option { value: "", "Select machining profile" }
                            for profile in snapshot.process_profiles.iter() {
                                option { value: "{profile.id}", "{profile.name}" }
                            }
                        }
                        p { class: "diag-status",
                            "Machining profile defines job bindings for CNC and fixture."
                        }
                        if let Some(active_profile) = snapshot.selected_process_profile() {
                            p { class: "diag-status",
                                {
                                    let cnc_name = snapshot
                                        .machines
                                        .iter()
                                        .find(|profile| profile.id == active_profile.cnc_profile_id)
                                        .map(|profile| profile.name.clone())
                                        .unwrap_or_else(|| {
                                            format!("Broken reference ({})", active_profile.cnc_profile_id)
                                        });
                                    let fixture_name = snapshot
                                        .fixtures
                                        .iter()
                                        .find(|profile| profile.id == active_profile.fixture_profile_id)
                                        .map(|profile| profile.name.clone())
                                        .unwrap_or_else(|| {
                                            format!("Broken reference ({})", active_profile.fixture_profile_id)
                                        });
                                    format!("Using CNC profile '{cnc_name}' and fixture '{fixture_name}'.")
                                }
                            }
                        }
                    }

                    if snapshot.selected_process_profile_id.is_none() {
                        p { class: "diag-status",
                            "Select a machining profile to display job attributes."
                        }
                    }

                    if snapshot.selected_process_profile_id.is_some() {
                        if let Some(active_profile) = snapshot.selected_process_profile() {
                            div { class: "field",
                                label { "Job summary" }
                                p { class: "diag-status", "Machining profile: {active_profile.name}" }
                                p { class: "diag-status",
                                    {
                                        let cnc_name = snapshot
                                            .machines
                                            .iter()
                                            .find(|profile| profile.id == active_profile.cnc_profile_id)
                                            .map(|profile| profile.name.clone())
                                            .unwrap_or_else(|| {
                                                format!("Broken reference ({})", active_profile.cnc_profile_id)
                                            });
                                        let fixture_name = snapshot
                                            .fixtures
                                            .iter()
                                            .find(|profile| profile.id == active_profile.fixture_profile_id)
                                            .map(|profile| profile.name.clone())
                                            .unwrap_or_else(|| {
                                                format!("Broken reference ({})", active_profile.fixture_profile_id)
                                            });
                                        let toolset_name = snapshot
                                            .toolsets
                                            .iter()
                                            .find(|profile| profile.id == active_profile.toolset_profile_id)
                                            .map(|profile| profile.name.clone())
                                            .unwrap_or_else(|| {
                                                format!("Broken reference ({})", active_profile.toolset_profile_id)
                                            });
                                        format!("CNC: {cnc_name} · Fixture: {fixture_name} · Toolset: {toolset_name}")
                                    }
                                }
                                p { class: "diag-status",
                                    {
                                        format!(
                                            "Side to machine: {}",
                                            if active_profile.side == Side::Bottom {
                                                "Bottom (Solder side)"
                                            } else {
                                                "Top (Component side)"
                                            },
                                        )
                                    }
                                }
                                p { class: "diag-status",
                                    "Operations: {snapshot.project_config.selected_operations.iter().map(|op| op.label()).collect::<Vec<_>>().join(\", \")}"
                                }
                                p { class: "diag-status",
                                    "Cut depth: {active_profile.cut_depth_strategy.label()}"
                                }
                                if active_profile.cut_depth_strategy == CutDepthStrategy::MultiPass {
                                    p { class: "diag-status",
                                        "Max depth/pass: {unit_format::format_length_display(active_profile.multi_pass_max_depth, snapshot.unit_system)}"
                                    }
                                }
                                p { class: "diag-status",
                                    if let Some(v) = board_thickness_pcb_label.as_ref() {
                                        "Board thickness (PCB): {v}"
                                    } else {
                                        "Board thickness (PCB): unavailable"
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
                                step: "0.1",
                                value: "{snapshot.project_config.rotation_angle}",
                                oninput: move |evt| {
                                    let value = evt.value().parse::<i32>().unwrap_or(0).clamp(-180, 180);
                                    crate::ui::screens::mutate_ctx(state, |s| s.project_config.rotation_angle = value);
                                },
                            }
                        }

                        if milling_outline_enabled {
                            details { class: "field collapsible-group",
                                summary { "Outline milling" }

                                div { class: "field section-subfield",
                                    label { "Router tool selection" }
                                    p { class: "diag-status", "Must be a router, diameter 0.8-2.5mm" }
                                    select {
                                        class: if router_ref_is_broken { "project-ref-select broken-ref-select" } else { "project-ref-select" },
                                        value: snapshot
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                .project_config
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                .outline_router_tool_id
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                .clone()
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                .unwrap_or_default(),
                                        onchange: move |evt| {
                                            let value = evt.value();
                                            state
                                                .with_mut(|s| {
                                                    s.project_config.outline_router_tool_id = if value.trim().is_empty() {
                                                        None
                                                    } else {
                                                        Some(value.clone())
                                                    };
                                                    let router_d = s

                                                        .project_config
                                                        .outline_router_tool_id
                                                        .as_ref()
                                                        .and_then(|id| s.tools.iter().find(|t| &t.id == id))
                                                        .map(|t| t.diameter.as_mm());
                                                    if let Some(drill_id) = s
                                                        .project_config
                                                        .mouse_bite_drill_tool_id
                                                        .clone()
                                                    {
                                                        let valid = s

                                                            .tools
                                                            .iter()
                                                            .find(|t| t.id == drill_id)
                                                            .map(|t| {
                                                                let kind_ok = t.kind.to_ascii_lowercase().contains("drill");
                                                                let d = t.diameter.as_mm();
                                                                let in_range = (0.5..=1.5).contains(&d);
                                                                let router_ok = router_d.map(|r| d <= r).unwrap_or(false);
                                                                kind_ok && in_range && router_ok
                                                            })
                                                            .unwrap_or(false);
                                                        if !valid {
                                                            s.project_config.mouse_bite_drill_tool_id = None;
                                                        }
                                                    }
                                                    s.validate_current_job_references();
                                                });
                                        },
                                        option { value: "", "Select router tool" }
                                        if router_ref_is_broken {
                                            option {
                                                value: "{snapshot.project_config.outline_router_tool_id.clone().unwrap_or_default()}",
                                                selected: true,
                                                "Broken reference ({snapshot.project_config.outline_router_tool_id.clone().unwrap_or_default()})"
                                            }
                                        }
                                        for tool in eligible_router_tools.iter() {
                                            option { value: "{tool.id}",
                                                "{tool.display_name()} ({tool.diameter})"
                                            }
                                        }
                                    }
                                }

                                div { class: "field section-subfield",
                                    label { "Number of tabs" }
                                    input {
                                        r#type: "number",
                                        min: "0",
                                        step: "1",
                                        value: "{snapshot.project_config.tab_count}",
                                        oninput: move |evt| {
                                            let value = evt.value().parse::<u8>().unwrap_or(0);
                                            crate::ui::screens::mutate_ctx(state, |s| s.project_config.tab_count = value);
                                        },
                                    }
                                }

                                if snapshot.project_config.tab_count > 0 {
                                    div { class: "field section-subfield",
                                        label { "Width of tabs" }
                                        p { class: "diag-status",
                                            "Recommended default: {tab_width_hint}"
                                        }
                                        div { class: "sub-field",
                                            input {
                                                r#type: "number",
                                                min: "0",
                                                step: "{tab_width_step}",
                                                value: "{tab_width_display}",
                                                oninput: move |evt| {
                                                    let value = evt.value().parse::<f32>().unwrap_or(0.0).max(0.0);
                                                    state
                                                        .with_mut(|s| {
                                                            s.project_config.tab_width = Length::from_mm(
                                                                unit_format::mm_from_display_length(
                                                                    value as f64,
                                                                    s.unit_system,
                                                                ),
                                                            );
                                                        });
                                                },
                                            }
                                            if tab_width_is_overridden {
                                                div { class: "stock-detail-original-group",
                                                    span { class: "stock-detail-original-value",
                                                        "{unit_format::format_length_display(snapshot.project_config.tab_width_baseline, snapshot.unit_system)}"
                                                    }
                                                    button {
                                                        r#type: "button",
                                                        class: "stock-detail-revert-btn",
                                                        title: "Revert to original setting",
                                                        onclick: move |_| {
                                                            state
                                                                .with_mut(|s| {
                                                                    s.project_config.tab_width = s
                                                                        .project_config
                                                                        .tab_width_baseline;
                                                                });
                                                        },
                                                        "↺"
                                                    }
                                                }
                                            }
                                        }
                                        p { class: "diag-status", "{tab_width_display_label}" }
                                    }

                                    div { class: "field section-subfield",
                                        label { "Mouse bites" }
                                        label { class: "checkbox-line",
                                            input {
                                                r#type: "checkbox",
                                                checked: snapshot.project_config.mouse_bites_enabled,
                                                oninput: move |evt| {
                                                    let enabled = evt.checked();
                                                    crate::ui::screens::mutate_ctx(state, |s| s.project_config.mouse_bites_enabled = enabled);
                                                },
                                            }
                                            span { "Enable mouse bites" }
                                        }
                                    }

                                    if snapshot.project_config.mouse_bites_enabled {
                                        div { class: "field section-subfield",
                                            label { "Center-to-center" }
                                            div { class: "sub-field",
                                                input {
                                                    r#type: "number",
                                                    min: "{mouse_bite_pitch_min}",
                                                    max: "{mouse_bite_pitch_max}",
                                                    step: "{tab_width_step}",
                                                    value: "{mouse_bite_pitch_display}",
                                                    oninput: move |evt| {
                                                        let value = evt.value().parse::<f32>().unwrap_or(0.8).max(0.0);
                                                        state
                                                            .with_mut(|s| {
                                                                s.project_config.mouse_bite_pitch = Length::from_mm(
                                                                    unit_format::mm_from_display_length(
                                                                        value as f64,
                                                                        s.unit_system,
                                                                    ),
                                                                );
                                                            });
                                                    },
                                                }
                                            }
                                            p { class: "diag-status",
                                                "{mouse_bite_pitch_display_label}"
                                            }
                                        }

                                        div { class: "field section-subfield",
                                            label { "Mouse-bite drill tool" }
                                            p { class: "diag-status",
                                                "Only drill bits 0.5-1.5mm, and not larger than selected router diameter"
                                            }
                                            select {
                                                class: if drill_ref_is_broken { "project-ref-select broken-ref-select" } else { "project-ref-select" },
                                                disabled: snapshot.project_config.outline_router_tool_id.is_none(),
                                                value: snapshot
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .project_config
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .mouse_bite_drill_tool_id
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .clone()
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        .unwrap_or_default(),
                                                onchange: move |evt| {
                                                    let value = evt.value();
                                                    state
                                                        .with_mut(|s| {
                                                            s.project_config.mouse_bite_drill_tool_id = if value.trim().is_empty()
                                                            {
                                                                None
                                                            } else {
                                                                Some(value)
                                                            };
                                                            s.validate_current_job_references();
                                                        });
                                                },
                                                option { value: "", "Select drill tool" }
                                                if drill_ref_is_broken {
                                                    option {
                                                        value: "{snapshot.project_config.mouse_bite_drill_tool_id.clone().unwrap_or_default()}",
                                                        selected: true,
                                                        "Broken reference ({snapshot.project_config.mouse_bite_drill_tool_id.clone().unwrap_or_default()})"
                                                    }
                                                }
                                                for tool in eligible_mouse_bite_drills.iter() {
                                                    option { value: "{tool.id}",
                                                        "{tool.display_name()} ({tool.diameter})"
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
}
