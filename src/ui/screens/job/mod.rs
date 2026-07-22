//! The Job screen: a tabbed workspace that applies the active PCB to a machining
//! profile. This module is the thin shell — the tab bar, view dispatch, and the
//! shared job-configuration sidebar — while each tab view lives in its own
//! sub-module ([`board`] PCB view, [`machining`], [`code`] G-code, [`rack`] slot
//! view) and the config panel in [`sidebar`].

use dioxus::prelude::*;

use crate::ui::navigation::*;

mod board;
mod code;
mod machining;
mod rack;
mod sidebar;
mod tooling;

use board::BoardView;
use code::CodeView;
use machining::MachiningView;
use rack::RackView;
use sidebar::JobSidebar;
use tooling::ToolingView;

#[component]
pub fn JobScreen(state: Signal<crate::runtime::AppCtx>) -> Element {
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

    rsx! {
        div { class: "screen single",
            div { class: "project-layout",
                section { class: "panel grow project-main",
                    div { class: "project-view-tabs",
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

                JobSidebar { state }
            }
        }
    }
}

