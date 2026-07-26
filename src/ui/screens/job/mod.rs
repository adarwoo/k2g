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

/// The tabbed Job view — the tab bar, the pin toggle, and the active tab's content.
///
/// Rendered in two places from this one definition: as the Job screen's main column,
/// and as the column docked beside a profile screen when the view is pinned. `docked`
/// only changes the chrome (the panel gains a caption naming what it is); the tabs
/// stay live either way, because switching Code ↔ Tooling while editing stock is most
/// of what the dock is for. Tab selection is the same `selected_job_view` state, so
/// returning to the Job screen lands on the tab left open in the dock.
#[component]
pub fn JobViewPanel(state: Signal<crate::runtime::AppCtx>, docked: bool) -> Element {
    let snapshot = state.read().clone();
    let has_atc = snapshot.selected_machine_has_atc();
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
    let pinned = snapshot.job_view_pinned;
    let pin_class = if pinned { "job-pin-toggle active" } else { "job-pin-toggle" };
    let pin_title = if pinned {
        "Unpin — stop showing this view on the profile screens"
    } else {
        "Pin — keep this view visible on the profile screens"
    };

    rsx! {
        section {
            class: if docked { "panel grow project-main job-panel-docked" } else { "panel grow project-main" },

            div { class: "project-view-tabs",
                // The tabs live in their own group so the pin is not a bare sibling of
                // the `for` loop — rsx drops a trailing element in that position.
                div { class: "project-view-tab-group",
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
                button {
                    class: "{pin_class}",
                    title: "{pin_title}",
                    onclick: move |_| super::mutate_ctx(state, |s| s.toggle_job_view_pinned()),
                    "📌"
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

