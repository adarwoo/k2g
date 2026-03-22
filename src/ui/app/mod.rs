use dioxus::prelude::*;

use super::boot_data;
use super::model::*;
use super::theme::APP_STYLE;

mod cnc;
mod job;
mod setup;
mod stock;

use cnc::CncScreen;
use job::JobScreen;
use setup::SetupScreen;
use stock::StockScreen;

#[component]
pub fn AppRoot() -> Element {
    let boot = boot_data().clone();
    let mut state = use_signal(|| {
        UiState::new(
            boot.save_filename_override.clone(),
            boot.board_snapshot.clone(),
        )
    });
    let mut show_error_details = use_signal(|| false);

    let snapshot = state.read().clone();
    let nav_screens = Screen::nav_items();
    let error_count = snapshot.errors.iter().filter(|e| e.is_error).count();
    let warning_count = snapshot.errors.len().saturating_sub(error_count);

    rsx! {
        style { "{APP_STYLE}" }

        div { class: if snapshot.theme == Theme::Dark { "app-shell theme-dark" } else { "app-shell theme-light" },
            if snapshot.show_first_launch {
                div { class: "wizard-overlay",
                    div { class: "wizard-dialog",
                        h2 { "Welcome to KiCad CNC Generator" }
                        p { "Create your first CNC profile to start using the plugin." }
                        div { class: "wizard-actions",
                            button {
                                class: "btn btn-primary",
                                onclick: move |_| state.with_mut(|s| s.add_demo_machine()),
                                "Create Demo Machine"
                            }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| {
                                    state
                                        .with_mut(|s| {
                                            s.show_first_launch = false;
                                            s.selected_screen = Screen::Setup;
                                        });
                                },
                                "Skip"
                            }
                        }
                    }
                }
            }

            div { class: "top-bar",
                div { class: "title", "k2g - KiCad to GCode" }
                div { class: "divider" }

                div { class: "top-control",
                    label { "CNC Profile" }
                    select {
                        disabled: snapshot.machines.is_empty(),
                        value: snapshot.selected_machine_id.clone().unwrap_or_default(),
                        onchange: move |evt| {
                            let value = evt.value();
                            state
                                .with_mut(|s| {
                                    if value.is_empty() {
                                        s.select_machine_profile_by_id(None);
                                    } else {
                                        s.select_machine_profile_by_id(Some(value));
                                    }
                                });
                        },
                        option { value: "", "Select CNC profile..." }
                        for machine in snapshot.machines.iter() {
                            option { value: machine.id.clone(), "{machine.name}" }
                        }
                    }
                }

                div { class: "spacer" }

                div { class: "status-line",
                    if snapshot.generation_state == GenerationState::Generating {
                        span { class: "status-pill status-busy", "Generating" }
                    } else if error_count == 0 && warning_count == 0 {
                        span { class: "status-pill status-ok", "Ready" }
                    } else {
                        span { class: "status-pill status-warn",
                            "{error_count} errors, {warning_count} warnings"
                        }
                    }

                    button {
                        class: "btn btn-icon",
                        onclick: move |_| state.with_mut(|s| s.select_screen(Screen::Setup)),
                        "Setup"
                    }
                }
            }

            if !snapshot.errors.is_empty() {
                div { class: "error-banner",
                    button {
                        class: "error-toggle",
                        onclick: move |_| {
                            let open = *show_error_details.read();
                            show_error_details.set(!open);
                        },
                        "{error_count} errors, {warning_count} warnings - click for details"
                    }

                    if *show_error_details.read() {
                        div { class: "error-list",
                            for err in snapshot.errors.iter() {
                                div { class: if err.is_error { "error-item error" } else { "error-item warning" },
                                    div { class: "error-title", "{err.message}" }
                                    if let Some(details) = err.details.as_ref() {
                                        div { class: "error-details", "{details}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            div { class: "work-area",
                aside { class: "left-nav",
                    for screen in nav_screens.iter() {
                        button {
                            key: "{screen.key()}",
                            class: if *screen == snapshot.selected_screen { "nav-item active" } else { "nav-item" },
                            onclick: {
                                let target = *screen;
                                move |_| state.with_mut(|s| s.select_screen(target))
                            },
                            "{screen.label()}"
                        }
                    }
                }

                main { class: "main-content",
                    match snapshot.selected_screen {
                        Screen::Setup => rsx! {
                            SetupScreen { state, boot: boot.clone() }
                        },
                        Screen::Job => rsx! {
                            JobScreen { state }
                        },
                        Screen::Stock => rsx! {
                            StockScreen { state }
                        },
                        Screen::Cnc => rsx! {
                            CncScreen { state }
                        },
                    }
                }
            }

            div { class: "footer-line",
                span { class: if boot.kicad_status.starts_with("Connected") { "kicad-ok" } else { "kicad-err" },
                    "KiCad: {boot.kicad_status}"
                }
                span { class: "env-summary", "{boot.env_summary}" }
            }
        }
    }
}
