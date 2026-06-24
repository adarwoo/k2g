use dioxus::prelude::*;
use serde_json::{json, Value};

use super::super::model::*;
use super::super::UiLaunchData;
use crate::config::save_global_settings;
use crate::ui::unit_service;
use crate::ui::persistence_state;
use crate::user_path::ensure_app_dirs;

#[component]
pub fn AppTopBar(
    state: Signal<UiState>,
    error_count: usize,
    warning_count: usize,
) -> Element {
    let snapshot = state.read().clone();
    let mut show_defaults = use_signal(|| false);

    let machine_name = snapshot
        .selected_machine()
        .map(|machine| machine.name.clone())
        .unwrap_or_else(|| "No CNC profile".to_string());
    let job_profile_name = snapshot
        .selected_job_profile()
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| "No job profile".to_string());
    let board_name = snapshot
        .board
        .as_ref()
        .map(|board| format!("Loaded board · {} holes", board.holes.len()))
        .unwrap_or_else(|| "No board loaded".to_string());
    let ops_label = if snapshot.job_config.selected_operations.is_empty() {
        "No ops".to_string()
    } else {
        format!("{} ops", snapshot.job_config.selected_operations.len())
    };
    let status_label = match snapshot.generation_state {
        GenerationState::Generating => "Generating".to_string(),
        GenerationState::Failed => "Generation failed".to_string(),
        GenerationState::Idle if error_count == 0 && warning_count == 0 => "Ready".to_string(),
        GenerationState::Idle => format!("{error_count} errors, {warning_count} warnings"),
    };

    rsx! {
        header { class: "shell-topbar",
            div { class: "brand-block",
                div { class: "brand-mark", "K" }
                div { class: "brand-copy",
                    div { class: "brand-title", "K2G" }
                    div { class: "brand-subtitle", "KiCad to GCode" }
                }
            }

            div { class: "topbar-board",
                span { class: "topbar-label", "Board" }
                span { class: "topbar-value mono", "{board_name}" }
            }

            div { class: "topbar-chip-row",
                SummaryChip { label: "CNC", value: machine_name }
                div { class: "unit-toggle",
                    button {
                        class: if snapshot.unit_system == UnitSystem::Metric { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            state.with_mut(|s| s.unit_system = UnitSystem::Metric);
                            persist_unit_system(UnitSystem::Metric);
                        },
                        "mm"
                    }
                    button {
                        class: if snapshot.unit_system == UnitSystem::Imperial { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            state.with_mut(|s| s.unit_system = UnitSystem::Imperial);
                            persist_unit_system(UnitSystem::Imperial);
                        },
                        "in"
                    }
                    button {
                        class: if snapshot.unit_system == UnitSystem::Mil { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            state.with_mut(|s| s.unit_system = UnitSystem::Mil);
                            persist_unit_system(UnitSystem::Mil);
                        },
                        "mil"
                    }
                }
                SummaryChip { label: "Profile", value: job_profile_name }
                SummaryChip { label: "Job", value: ops_label }
            }

            div { class: "shell-spacer" }

            div { class: "topbar-status-group",
                span {
                    class: match snapshot.generation_state {
                        GenerationState::Generating => "status-pill status-busy",
                        GenerationState::Failed => "status-pill status-err",
                        GenerationState::Idle if error_count == 0 && warning_count == 0 => {
                            "status-pill status-ok"
                        }
                        GenerationState::Idle => "status-pill status-warn",
                    },
                    "{status_label}"
                }

                button {
                    class: "icon-button",
                    onclick: move |_| {
                        let next = if snapshot.theme == Theme::Dark {
                            Theme::Light
                        } else {
                            Theme::Dark
                        };
                        state.with_mut(|s| s.theme = next);
                        persist_theme(next);
                    },
                    if snapshot.theme == Theme::Dark {
                        "Theme: Dark"
                    } else {
                        "Theme: Light"
                    }
                }

                button {
                    class: "icon-button",
                    onclick: move |_| show_defaults.set(true),
                    "Defaults"
                }
            }

            if *show_defaults.read() {
                div { class: "wizard-overlay",
                    div { class: "wizard-dialog",
                        h3 { "Default settings" }
                        p { "Theme and units are global defaults and are persisted." }

                        div { class: "field",
                            label { "Theme" }
                            select {
                                value: snapshot.theme.as_str(),
                                onchange: move |evt| {
                                    let selected = Theme::from_str(&evt.value());
                                    state.with_mut(|s| s.theme = selected);
                                    persist_theme(selected);
                                },
                                option { value: "dark", "Dark" }
                                option { value: "light", "Light" }
                            }
                        }

                        div { class: "field",
                            label { "Display units" }
                            div { class: "unit-toggle",
                                button {
                                    class: if snapshot.unit_system == UnitSystem::Metric { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                                    onclick: move |_| {
                                        state.with_mut(|s| s.unit_system = UnitSystem::Metric);
                                        persist_unit_system(UnitSystem::Metric);
                                    },
                                    "mm"
                                }
                                button {
                                    class: if snapshot.unit_system == UnitSystem::Imperial { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                                    onclick: move |_| {
                                        state.with_mut(|s| s.unit_system = UnitSystem::Imperial);
                                        persist_unit_system(UnitSystem::Imperial);
                                    },
                                    "in"
                                }
                                button {
                                    class: if snapshot.unit_system == UnitSystem::Mil { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                                    onclick: move |_| {
                                        state.with_mut(|s| s.unit_system = UnitSystem::Mil);
                                        persist_unit_system(UnitSystem::Mil);
                                    },
                                    "mil"
                                }
                            }
                        }

                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-primary",
                                onclick: move |_| show_defaults.set(false),
                                "Close"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SummaryChip(label: String, value: String) -> Element {
    rsx! {
        div { class: "summary-chip",
            span { class: "summary-chip-label", "{label}" }
            span { class: "summary-chip-value", "{value}" }
        }
    }
}

#[component]
pub fn DiagnosticsBanner(
    errors: Vec<AppError>,
    generation_state: GenerationState,
    show_error_details: Signal<bool>,
) -> Element {
    if errors.is_empty() {
        return rsx! {};
    }

    let error_count = errors.iter().filter(|entry| entry.is_error).count();
    let warning_count = errors.len().saturating_sub(error_count);
    let banner_class = if error_count > 0 {
        "diag-banner diag-banner-error"
    } else {
        "diag-banner diag-banner-warning"
    };
    let status_text = match generation_state {
        GenerationState::Generating => "Generation in progress",
        GenerationState::Failed => "Generation failed",
        GenerationState::Idle => "Diagnostics available",
    };

    rsx! {
        div { class: "diag-banner-wrap",
            div { class: banner_class,
                div { class: "diag-banner-main",
                    span { class: "diag-banner-dot" }
                    div { class: "diag-banner-copy",
                        div { class: "diag-banner-title",
                            "{error_count} errors, {warning_count} warnings"
                        }
                        div { class: "diag-banner-subtitle", "{status_text}" }
                    }
                }
                button {
                    class: "text-button",
                    onclick: move |_| {
                        let is_open = *show_error_details.read();
                        show_error_details.set(!is_open);
                    },
                    if *show_error_details.read() {
                        "Hide details"
                    } else {
                        "Show details"
                    }
                }
            }

            if *show_error_details.read() {
                div { class: "diag-detail-list",
                    for err in errors.iter() {
                        article { class: if err.is_error { "diag-detail-card is-error" } else { "diag-detail-card is-warning" },
                            div { class: "diag-detail-title", "{err.message}" }
                            if let Some(details) = err.details.as_ref() {
                                div { class: "diag-detail-text", "{details}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn NavigationRail(state: Signal<UiState>) -> Element {
    let snapshot = state.read().clone();
    let nav_items = [
        Screen::Job,
        Screen::CncProfiles,
        Screen::FixtureProfiles,
        Screen::JobProfiles,
        Screen::Stock,
        Screen::Catalog,
    ];

    rsx! {
        aside { class: "shell-rail",
            for screen in nav_items {
                button {
                    key: "{screen.key()}",
                    class: if screen == snapshot.selected_screen { "rail-button active" } else { "rail-button" },
                    onclick: move |_| state.with_mut(|s| s.select_screen(screen)),
                    span { class: "rail-button-content",
                        span { class: "rail-button-icon", {rail_icon(screen)} }
                        span { class: "rail-button-text", "{screen.label()}" }
                    }
                }
            }
        }
    }
}

fn rail_icon(screen: Screen) -> Element {
    match screen {
        Screen::Job => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M4 6h16v12H4z" }
                path { d: "M4 10h16" }
                circle { cx: "8", cy: "8", r: "1" }
                circle { cx: "12", cy: "8", r: "1" }
                circle { cx: "16", cy: "8", r: "1" }
            }
        },
        Screen::CncProfiles => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M12 4l5 3v6l-5 3-5-3V7z" }
                path { d: "M12 16v4" }
                path { d: "M9.5 20h5" }
            }
        },
        Screen::FixtureProfiles => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect {
                    x: "4",
                    y: "5",
                    width: "16",
                    height: "14",
                    rx: "2",
                }
                path { d: "M8 5v14" }
                path { d: "M16 5v14" }
            }
        },
        Screen::JobProfiles => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect {
                    x: "5",
                    y: "4",
                    width: "10",
                    height: "13",
                    rx: "1.8",
                }
                path { d: "M8 8h4" }
                path { d: "M8 11h4" }
                path { d: "M15 9l4 4-4 4" }
            }
        },
        Screen::Stock => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect {
                    x: "4",
                    y: "7",
                    width: "16",
                    height: "10",
                    rx: "2",
                }
                path { d: "M8 7V5" }
                path { d: "M16 7V5" }
                path { d: "M8 12h8" }
            }
        },
        Screen::Catalog => rsx! {
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M6 5h12v14H6z" }
                path { d: "M9 8h6" }
                path { d: "M9 11h6" }
                path { d: "M9 14h4" }
            }
        },
    }
}

#[component]
pub fn StatusBar(state: Signal<UiState>, boot: UiLaunchData) -> Element {
    let snapshot = state.read().clone();
    let board_label = snapshot
        .board
        .as_ref()
        .map(|board| format!("{} holes · {} edges", board.holes.len(), board.edge_shapes.len()))
        .unwrap_or_else(|| "No board snapshot".to_string());
    let generation_label = match snapshot.generation_state {
        GenerationState::Generating => "Generating GCode…".to_string(),
        GenerationState::Failed => "Last generation failed".to_string(),
        GenerationState::Idle => {
            let modified = if snapshot.gcode_modified { "modified" } else { "clean" };
            format!("{} · {}", snapshot.save_filename, modified)
        }
    };

    rsx! {
        footer { class: "shell-statusbar",
            span { class: if boot.kicad_status.starts_with("Connected") { "status-connection ok" } else { "status-connection err" },
                "KiCad: {boot.kicad_status}"
            }
            span { class: "status-meta", "{board_label}" }
            span { class: "status-meta", "{generation_label}" }
            span { class: "status-summary", "{boot.env_summary}" }
        }
    }
}

fn persist_unit_system(unit_system: UnitSystem) {
    let Ok(app_dirs) = ensure_app_dirs() else {
        return;
    };

    let mut global_settings = persistence_state()
        .map(|state| state.global_settings.clone())
        .unwrap_or_else(|| json!({}));

    if !global_settings.is_object() {
        global_settings = json!({});
    }

    let Some(root) = global_settings.as_object_mut() else {
        return;
    };

    let units_value = root.entry("units".to_string()).or_insert_with(|| json!({}));
    if !units_value.is_object() {
        *units_value = json!({});
    }

    let Some(units) = units_value.as_object_mut() else {
        return;
    };

    units.insert(
        "system".to_string(),
        Value::String(unit_system.as_str().to_string()),
    );
    if !units.contains_key("size_unit") {
        units.insert(
            "size_unit".to_string(),
            Value::String(match unit_system {
                UnitSystem::Metric => "mm".to_string(),
                UnitSystem::Imperial => "in".to_string(),
                UnitSystem::Mil => "mil".to_string(),
            }),
        );
    }
    if !units.contains_key("speed_unit") {
        units.insert(
            "speed_unit".to_string(),
            Value::String(unit_service::feed_unit_label(unit_system).to_string()),
        );
    }

    let _ = save_global_settings(&app_dirs, &global_settings);
}

fn persist_theme(theme: Theme) {
    let Ok(app_dirs) = ensure_app_dirs() else {
        return;
    };

    let mut global_settings = persistence_state()
        .map(|state| state.global_settings.clone())
        .unwrap_or_else(|| json!({}));

    if !global_settings.is_object() {
        global_settings = json!({});
    }

    let Some(root) = global_settings.as_object_mut() else {
        return;
    };

    let theme_value = root.entry("theme".to_string()).or_insert_with(|| json!({}));
    if !theme_value.is_object() {
        *theme_value = json!({});
    }

    let Some(theme_root) = theme_value.as_object_mut() else {
        return;
    };

    theme_root.insert("mode".to_string(), Value::String(theme.as_str().to_string()));

    let _ = save_global_settings(&app_dirs, &global_settings);
}