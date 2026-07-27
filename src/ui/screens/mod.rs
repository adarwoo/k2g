use dioxus::prelude::*;

use crate::ui::navigation::*;
use super::theme::APP_STYLE;
use crate::runtime::{ctx_snapshot, with_ctx_mut, MAX_JOB_PIN_WIDTH, MIN_JOB_PIN_WIDTH};

mod about;
mod cnc;
mod catalog;
mod fixture;
mod logs;
mod profile_manager;
mod profiles_common;
mod job;
mod machining;
mod save_program;
mod shell;
mod stock;
mod toolset;

use about::AboutScreen;
use cnc::CncScreen;
use catalog::CatalogScreen;
use fixture::FixtureProfilesScreen;
use logs::LogsScreen;
use job::{JobScreen, JobViewPanel};
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

    // Split-handle state. The live width is local to the drag so the pointer stays
    // glued to the divider; it is written back to settings once, on release.
    let mut dock_dragging = use_signal(|| false);
    let mut dock_drag_last = use_signal(|| 0.0_f64);
    let mut dock_live_width = use_signal(|| snapshot.job_pin_width as f64);
    // Adopt the persisted width whenever it changes underneath us (launch, or a
    // settings write from elsewhere) — but never mid-drag, which would fight the
    // pointer.
    if !*dock_dragging.read() && *dock_live_width.peek() != snapshot.job_pin_width as f64 {
        dock_live_width.set(snapshot.job_pin_width as f64);
    }
    let dock_width = *dock_live_width.read();

    // The dock appears only where it can do something: pinned, and on a screen whose
    // edits actually feed the plan. Gating the *render* here means an unpinned session
    // pays nothing for the feature; the narrow-window case is handled in CSS.
    let show_dock = snapshot.job_view_pinned && snapshot.selected_screen.shows_pinned_job();

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
                    // Docked Job view. The stored width rides in as a custom property
                    // so the stylesheet keeps ownership of the layout — an inline
                    // `grid-template-columns` would outrank the media query that
                    // collapses the dock on a narrow window.
                    div {
                        class: if show_dock { "dock-layout is-docked" } else { "dock-layout" },
                        style: "--job-dock-width: {dock_width}px;",
                        onmousemove: move |evt| {
                            if !*dock_dragging.read() {
                                return;
                            }
                            // Track the delta in client space: the pointer leaves the
                            // thin handle almost immediately, and element-relative
                            // coordinates would jump as the target changes under it.
                            let x = evt.client_coordinates().x;
                            let last = *dock_drag_last.read();
                            dock_drag_last.set(x);
                            let next = (*dock_live_width.read() + (x - last)) as i64;
                            dock_live_width
                                .set(next.clamp(MIN_JOB_PIN_WIDTH, MAX_JOB_PIN_WIDTH) as f64);
                        },
                        onmouseup: move |_| {
                            if !*dock_dragging.read() {
                                return;
                            }
                            dock_dragging.set(false);
                            // One settings write per drag, on release.
                            let width = *dock_live_width.read() as i64;
                            mutate_ctx(state, |ctx| ctx.app.set_job_pin_width(width));
                        },
                        onmouseleave: move |_| dock_dragging.set(false),

                        if show_dock {
                            JobViewPanel { state, docked: true }
                            div {
                                class: if *dock_dragging.read() { "dock-handle is-dragging" } else { "dock-handle" },
                                title: "Drag to resize the pinned Job view",
                                onmousedown: move |evt| {
                                    dock_drag_last.set(evt.client_coordinates().x);
                                    dock_dragging.set(true);
                                },
                            }
                        }

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
            }

            EventNotifications { state }

            StatusBar { state }
        }
    }
}


