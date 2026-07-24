use dioxus::prelude::*;

use crate::ui::navigation::*;
use super::theme::APP_STYLE;
use crate::runtime::{ctx_snapshot, with_ctx_mut};

mod about;
mod cnc;
mod catalog;
mod fixture;
mod logs;
mod profile_manager;
mod profiles_common;
mod job;
mod machining;
mod shell;
mod stock;
mod toolset;

use about::AboutScreen;
use cnc::CncScreen;
use catalog::CatalogScreen;
use fixture::FixtureProfilesScreen;
use logs::LogsScreen;
use job::JobScreen;
use machining::MachiningProfilesScreen;
use shell::{AppTopBar, DiagnosticsBanner, EventNotifications, NavigationRail, StatusBar};
use stock::StockScreen;
use toolset::ToolsetProfilesScreen;

pub fn mutate_ctx<R>(mut state: Signal<crate::runtime::AppCtx>, f: impl FnOnce(&mut crate::runtime::AppCtx) -> R) -> R {
    let result = with_ctx_mut(f);
    state.set(ctx_snapshot());
    result
}

#[component]
pub fn AppRoot() -> Element {
    let state = use_signal(ctx_snapshot);
    let show_error_details = use_signal(|| false);

    // Bridge background generation → UI. The worker publishes results into the
    // global ctx off the UI thread and bumps a wake channel; re-sync the signal on
    // each bump so the Job views refresh without a user action. (The startup board
    // comes from the boot payload via `from_launch`; the Reload PCB action
    // re-acquires on demand — see `docs/gcode-generation.md` §4, §8.)
    use_future(move || async move {
        let mut state = state;
        if let Some(mut wake) = crate::runtime::ui_wake_receiver() {
            while wake.changed().await.is_ok() {
                state.set(ctx_snapshot());
            }
        }
    });

    let snapshot = state.read().clone();

    rsx! {
        style { "{APP_STYLE}" }

        div { class: if snapshot.theme == Theme::Dark { "app-shell shell-theme-dark" } else { "app-shell shell-theme-light" },
            AppTopBar { state }

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
                            Screen::Logs => rsx! {
                                LogsScreen { state }
                            },
                            Screen::About => rsx! {
                                AboutScreen { state }
                            },
                        }
                    }
                }
            }

            EventNotifications { state }

            StatusBar { state }
        }
    }
}


