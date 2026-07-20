use dioxus::prelude::*;

use super::boot_data;
use crate::domain::*;
use super::theme::APP_STYLE;
use crate::app_state_impl::{ctx_snapshot, with_ctx_mut};
use pcb::KiCad;

mod cnc;
mod catalog;
mod fixture;
mod profile_manager;
mod profiles_common;
mod job;
mod machining;
mod shell;
mod stock;
mod toolset;

use cnc::CncScreen;
use catalog::CatalogScreen;
use fixture::FixtureProfilesScreen;
use job::JobScreen;
use machining::MachiningProfilesScreen;
use shell::{AppTopBar, DiagnosticsBanner, EventNotifications, NavigationRail, StatusBar};
use stock::StockScreen;
use toolset::ToolsetProfilesScreen;

pub fn mutate_ctx<R>(mut state: Signal<crate::app_state_impl::AppCtx>, f: impl FnOnce(&mut crate::app_state_impl::AppCtx) -> R) -> R {
    let result = with_ctx_mut(f);
    state.set(ctx_snapshot());
    result
}

#[component]
pub fn AppRoot() -> Element {
    let boot = boot_data().clone();
    let state = use_signal(ctx_snapshot);
    let show_error_details = use_signal(|| false);
    let mut startup_board_sync_done = use_signal(|| false);

    // Auto-load board on startup
    use_effect(move || {
        if !*startup_board_sync_done.read() {
            startup_board_sync_done.set(true);
            match KiCad::connect() {
                Ok(client) => {
                    if let Ok(pcbs) = client.enumerate_pcbs() {
                        if let Some(pcb) = pcbs.first() {
                            if let Ok(board_snapshot) = client.collect_snapshot(pcb) {
                                mutate_ctx(state, |s| s.board = Some(board_snapshot));
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


