//! The Job screen: a tabbed workspace that applies the active PCB to a machining
//! profile. This module is the thin shell — the tab bar, view dispatch, and the
//! shared job-configuration sidebar — while each tab view lives in its own
//! sub-module ([`board`] PCB view, [`machining`], [`code`] G-code, [`rack`] slot
//! view) and the config panel in [`sidebar`].

use dioxus::prelude::*;

use crate::ui::navigation::*;

mod board;
mod code;
mod gcode_highlight;
mod machining;
mod machining_3d;
mod rack;
mod sidebar;
mod tooling;

use board::BoardView;
use code::CodeView;
use machining::MachiningView;
use rack::RackView;
use sidebar::JobSidebar;
use tooling::ToolingView;

/// The tabbed Job view — the tab bar and the active tab's content.
///
/// Rendered in two places from this one definition: as the Job screen's main column,
/// and as the column docked beside a profile screen when the view is pinned. `docked`
/// only changes the chrome (the panel gains a caption naming what it is); the tabs
/// stay live either way, because switching Code ↔ Tooling while editing stock is most
/// of what the dock is for. Tab selection is the same `selected_job_view` state, so
/// returning to the Job screen lands on the tab left open in the dock.
///
/// The pin that controls the dock is NOT here: it lives beside Job in the navigation
/// rail, so it can be reached from whichever screen the user wants the dock on rather
/// than only after navigating to the thing being pinned.
#[component]
pub fn JobViewPanel(state: Signal<crate::runtime::AppCtx>, docked: bool) -> Element {
    let snapshot = state.read().clone();
    // Every view below shows one step. The steps come from the datastore rather than from
    // a plan so the chrome does not vanish when there is no board to plan against.
    let steps = crate::runtime::tooling::step_headers(&snapshot);
    let selected_step = snapshot.selected_step.min(steps.len().saturating_sub(1));

    // Rack is offered when *any* step runs on a machine with a tool changer — "Rack when
    // relevant" (Specification §8.4), asked of the job rather than of the selected step.
    // Asking the selected step made the tab come and go as the step chips were clicked,
    // and clicking a chip while reading the rack silently dropped the user onto the board
    // view. Which of the job's racks is shown, and what a step with no rack says instead,
    // is [`RackView`]'s business.
    let has_atc = steps.iter().any(|step| step.is_atc);
    let mut views =
        vec![JobCenterView::Board, JobCenterView::Machining, JobCenterView::Code, JobCenterView::Tooling];
    if has_atc {
        views.push(JobCenterView::Rack);
    }

    let active_view = if snapshot.selected_job_view == JobCenterView::Rack && !has_atc {
        JobCenterView::Board
    } else {
        snapshot.selected_job_view
    };

    // A one-step job must show no trace of steps, so the whole row is absent rather than
    // rendered with a single chip.
    let show_steps = steps.len() > 1;
    rsx! {
        section {
            class: if docked { "panel grow project-main job-panel-docked" } else { "panel grow project-main" },

            // The caption comes first and the tabs trail it: rsx drops a trailing
            // element that follows a `for` loop, so nothing may be added after the
            // loop here without wrapping it.
            div { class: "project-view-tabs",
                if docked {
                    span { class: "job-dock-caption", "Job" }
                }
                for view in views.iter() {
                    button {
                        key: "{view.key()}",
                        class: if *view == active_view { "project-view-tab active" } else { "project-view-tab" },
                        onclick: {
                            let target = *view;
                            move |_| super::mutate_ctx(state, |s| s.selected_job_view = target)
                        },
                        "{view.label()}"
                    }
                }
            }

            if show_steps {
                div { class: "project-step-chips",
                    for step in steps.iter() {
                        {
                            let index = step.index;
                            let failed = snapshot
                                .programs
                                .get(index)
                                .and_then(|program| program.failure())
                                .map(str::to_string);
                            let label = if step.name.trim().is_empty() {
                                format!("Step {}", index + 1)
                            } else {
                                step.name.clone()
                            };
                            rsx! {
                                button {
                                    key: "{index}",
                                    // A step whose program failed says so here, so the
                                    // failure is discoverable without opening Code.
                                    class: match (index == selected_step, failed.is_some()) {
                                        (true, true) => "project-step-chip active has-error",
                                        (true, false) => "project-step-chip active",
                                        (false, true) => "project-step-chip has-error",
                                        (false, false) => "project-step-chip",
                                    },
                                    title: failed.clone().unwrap_or_else(|| step.cnc_name.clone()),
                                    onclick: move |_| super::mutate_ctx(state, |s| s.selected_step = index),
                                    span { class: "project-step-chip-index", "{index + 1}" }
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }

            match active_view {
                JobCenterView::Board => rsx! {
                    BoardView { state }
                },
                JobCenterView::Machining => rsx! {
                    MachiningView { state }
                },
                JobCenterView::Code => rsx! {
                    CodeView { state }
                },
                JobCenterView::Tooling => rsx! {
                    ToolingView { state }
                },
                JobCenterView::Rack => rsx! {
                    RackView { state }
                },
            }
        }
    }
}

#[component]
pub fn JobScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
    rsx! {
        div { class: "screen single",
            div { class: "project-layout",
                JobViewPanel { state, docked: false }
                JobSidebar { state }
            }
        }
    }
}

