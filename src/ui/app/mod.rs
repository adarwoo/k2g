use dioxus::prelude::*;

use super::boot_data;
use super::model::*;
use super::theme::APP_STYLE;
use crate::board::collect_board_snapshot_for_board;
use crate::ctx::{ctx_snapshot, sync_ctx_from_ui_state_and_persist};
use crate::kicad_wrapper::KiCadClientBlocking;
use kicad_ipc_rs::DocumentType;

mod cnc;
mod catalog;
mod fixture;
mod profiles_common;
#[path = "project.rs"]
mod job;
mod setup;
mod setup_sections;
#[path = "process_profiles.rs"]
mod machining_profiles;
mod shell;
mod stock;
mod toolset;

use cnc::CncScreen;
use catalog::CatalogScreen;
use fixture::FixtureProfilesScreen;
use job::JobScreen;
use machining_profiles::MachiningProfilesScreen;
use shell::{AppTopBar, DiagnosticsBanner, EventNotifications, NavigationRail, StatusBar};
use stock::StockScreen;
use toolset::ToolsetProfilesScreen;

#[component]
pub fn AppRoot() -> Element {
    let boot = boot_data().clone();
    let mut state = use_signal(ctx_snapshot);
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
                                state.with_mut(|s| s.ui.board = Some(board_snapshot));
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

    // Persist all mutable configuration domains automatically.
    use_effect(move || {
        let snapshot = state.read().clone();
        sync_ctx_from_ui_state_and_persist(&snapshot.ui);
    });

    let snapshot = state.read().clone().ui;
    let error_count = snapshot.errors.iter().filter(|e| e.is_error).count();
    let warning_count = snapshot.errors.len().saturating_sub(error_count);

    rsx! {
        style { "{APP_STYLE}" }

        div { class: if snapshot.theme == Theme::Dark { "app-shell shell-theme-dark" } else { "app-shell shell-theme-light" },
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
                            Screen::Job => rsx! {
                                JobScreen { state }
                            },
                            Screen::CncProfiles => rsx! {
                                CncScreen { state }
                            },
                            Screen::FixtureProfiles => rsx! {
                                FixtureProfilesScreen { state }
                            },
                            Screen::MachiningProfiles => rsx! {
                                MachiningProfilesScreen { state }
                            },
                            Screen::ToolsetProfiles => rsx! {
                                ToolsetProfilesScreen { state }
                            },
                            Screen::Stock => rsx! {
                                StockScreen { state }
                            },
                            Screen::Catalog => rsx! {
                                CatalogScreen { state }
                            },
                        }
                    }
                }
            }

            EventNotifications { state }

            StatusBar { state, boot: boot.clone() }
        }
    }
}


