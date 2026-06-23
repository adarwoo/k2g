use dioxus::prelude::*;

use super::boot_data;
use super::model::*;
use super::theme::APP_STYLE;
use crate::board::collect_board_snapshot_for_board;
use kicad_ipc_rs::{DocumentType, KiCadClientBlocking};

mod cnc;
mod job;
mod setup;
mod setup_sections;
mod shell;
mod stock;

use cnc::CncScreen;
use job::JobScreen;
use shell::{AppTopBar, DiagnosticsBanner, NavigationRail, StatusBar};
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
    let show_error_details = use_signal(|| false);
    let mut startup_board_sync_done = use_signal(|| false);

    // Auto-load board on startup
    use_effect(move || {
        if !*startup_board_sync_done.read() {
            startup_board_sync_done.set(true);
            match KiCadClientBlocking::connect() {
                Ok(client) => {
                    if let Ok(docs) = client.get_open_documents(DocumentType::Pcb) {
                        let mut boards: Vec<String> = docs
                            .into_iter()
                            .filter_map(|doc| doc.board_filename)
                            .collect();
                        boards.sort();
                        boards.dedup();
                        if !boards.is_empty() {
                            if let Ok(board_snapshot) = collect_board_snapshot_for_board(&client, Some(&boards[0])) {
                                state.with_mut(|s| s.board = Some(board_snapshot));
                            }
                        }
                    }
                }
                Err(_) => {
                    // KiCad not available - that's OK, board will be unavailable
                }
            }
        }
    });

    let snapshot = state.read().clone();
    let error_count = snapshot.errors.iter().filter(|e| e.is_error).count();
    let warning_count = snapshot.errors.len().saturating_sub(error_count);

    rsx! {
        style { "{APP_STYLE}" }

        div { class: if snapshot.theme == Theme::Dark { "app-shell shell-theme-dark" } else { "app-shell shell-theme-light" },
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

            AppTopBar { state, error_count, warning_count }

            DiagnosticsBanner {
                errors: snapshot.errors.clone(),
                generation_state: snapshot.generation_state,
                show_error_details,
            }

            div { class: "shell-body",
                NavigationRail { state }

                main { class: "shell-content",
                    div { class: "screen-host",
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
            }

            StatusBar { state, boot: boot.clone() }
        }
    }
}
