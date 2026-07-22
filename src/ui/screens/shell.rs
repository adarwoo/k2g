use dioxus::prelude::*;
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

use crate::data::model::*;
use crate::ui::navigation::*;
use crate::runtime::AppError;
use crate::runtime::{UiCommand, apply_ui_command, ctx_snapshot, with_ctx_mut};

#[component]
pub fn AppTopBar(
    state: Signal<crate::runtime::AppCtx>,
    error_count: usize,
    warning_count: usize,
) -> Element {
    let snapshot = state.read().clone();

    let has_board = snapshot.board.is_some();
    let has_process_profile = snapshot.selected_process_profile().is_some();
    let has_machining_operation = !snapshot.project_config.selected_operations.is_empty();

    let process_profile_name = snapshot
        .selected_process_profile()
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| "No machining profile selected".to_string());
    let board_name = snapshot
        .board
        .as_ref()
        .map(|board| {
            if board.name.is_empty() {
                "Loaded board".to_string()
            } else {
                board.name.clone()
            }
        })
        .unwrap_or_else(|| "No board loaded".to_string());
    let status_label = match snapshot.generation_state {
        GenerationState::Running => "Generating…".to_string(),
        GenerationState::Failed => "Generation failed".to_string(),
        GenerationState::Idle if error_count == 0 && warning_count == 0 => "Ready".to_string(),
        GenerationState::Idle => format!("{error_count} errors, {warning_count} warnings"),
    };

    rsx! {
        header { class: "shell-topbar",
            div { class: "brand-block",
                img {
                    class: "brand-mark-image",
                    src: app_icon_data_url(),
                    alt: "K2G",
                }
                div { class: "brand-copy",
                    div { class: "brand-title", "K2G" }
                    div { class: "brand-subtitle", "KiCad to GCode" }
                }
            }

            div { class: "topbar-board",
                span { class: "topbar-label", "Board" }
                div { class: "topbar-board-row",
                    // The reachable KiCad's open board (at most one — see the
                    // `kicad-multi-instance` reference), plus a refresh glyph.
                    span {
                        class: if has_board { "topbar-value mono" } else { "topbar-value topbar-value-missing mono" },
                        "{board_name}"
                    }
                    button {
                        class: "board-reload-btn",
                        r#type: "button",
                        title: "Refresh PCB data",
                        "aria-label": "Refresh PCB data",
                        onclick: move |_| do_refresh(state),
                        "\u{21bb}"
                    }
                }
            }

            div { class: "topbar-board",
                span { class: "topbar-label", "Job" }
                span { class: if has_process_profile { "topbar-value mono" } else { "topbar-value topbar-value-missing mono" },
                    "{process_profile_name}"
                }
                if !has_machining_operation {
                    span { class: "topbar-value topbar-value-missing", "No machining operation selected" }
                }
            }

            div { class: "topbar-chip-row",
                div { class: "unit-toggle",
                    button {
                        class: if snapshot.unit_system == UserUnitSystem::Metric { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            dispatch_ui_command(state, UiCommand::SetUnitSystem(UserUnitSystem::Metric));
                        },
                        "mm"
                    }
                    button {
                        class: if snapshot.unit_system == UserUnitSystem::Imperial { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            dispatch_ui_command(state, UiCommand::SetUnitSystem(UserUnitSystem::Imperial));
                        },
                        "in"
                    }
                    button {
                        class: if snapshot.unit_system == UserUnitSystem::Mil { "unit-toggle-btn active" } else { "unit-toggle-btn" },
                        onclick: move |_| {
                            dispatch_ui_command(state, UiCommand::SetUnitSystem(UserUnitSystem::Mil));
                        },
                        "mil"
                    }
                }
            }

            div { class: "shell-spacer" }

            div { class: "topbar-status-group",
                span {
                    class: match snapshot.generation_state {
                        GenerationState::Running => "status-pill status-warn",
                        GenerationState::Failed => "status-pill status-warn",
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
                        dispatch_ui_command(state, UiCommand::ToggleTheme);
                    },
                    if snapshot.theme == Theme::Dark {
                        "Theme: Dark"
                    } else {
                        "Theme: Light"
                    }
                }
            }
        }
    }
}

/// Re-acquire the reachable KiCad's board (recovering a connection made after
/// startup) and update the status. Setting a changed board re-stitches once and
/// triggers regeneration (see `sync_after_mutation`).
fn do_refresh(state: Signal<crate::runtime::AppCtx>) {
    let (status, board) = crate::runtime::acquire_board();
    super::mutate_ctx(state, |s| {
        s.kicad_status = status;
        s.board = board;
    });
}

fn dispatch_ui_command(mut state: Signal<crate::runtime::AppCtx>, command: UiCommand) {
    // Stock and other screens may mutate the local signal directly. Ensure
    // the global context is up to date before applying global UI commands.
    let latest_snapshot = state.read().clone();
    with_ctx_mut(|ctx| *ctx = latest_snapshot);

    apply_ui_command(command);
    state.set(ctx_snapshot());
    // Datastore-backed fields (SchemaField) read the active unit system from the
    // live context; nudge their render counter so they reconvert on unit change.
    crate::ui::bindings::bump_render();
}

fn app_icon_data_url() -> &'static str {
    static ICON_DATA_URL: OnceLock<String> = OnceLock::new();

    ICON_DATA_URL.get_or_init(|| {
        let icon_bytes = include_bytes!("../../../assets/icons/icon.png");
        format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(icon_bytes)
        )
    })
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
        GenerationState::Running => "Generating…",
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
pub fn NavigationRail(state: Signal<crate::runtime::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let nav_items = [
        Some(Screen::Job),
        None,
        Some(Screen::MachiningProfiles),
        Some(Screen::CncProfiles),
        Some(Screen::FixtureProfiles),
        Some(Screen::ToolsetProfiles),
        None,
        Some(Screen::Stock),
        Some(Screen::Catalog),
    ];

    rsx! {
        aside { class: "shell-rail",
            for (idx , item) in nav_items.iter().enumerate() {
                if let Some(screen) = *item {
                    button {
                        key: "{screen.key()}",
                        class: if screen == snapshot.selected_screen { "rail-button active" } else { "rail-button" },
                        onclick: move |_| super::mutate_ctx(state, |s| s.select_screen(screen)),
                        span { class: "rail-button-content",
                            span { class: "rail-button-icon", {rail_icon(screen)} }
                            span { class: "rail-button-text", "{screen.label()}" }
                        }
                    }
                } else {
                    div { key: "sep-{idx}", class: "rail-separator" }
                }
            }
        }
    }
}

fn rail_icon(screen: Screen) -> Element {
    match screen {
        Screen::Job => rsx! {
            // Circuit board: an IC with legs — the PCB the job produces.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "3", y: "4", width: "18", height: "16", rx: "2" }
                rect { x: "9", y: "9", width: "6", height: "6", rx: "1" }
                path { d: "M9 11H6" }
                path { d: "M9 13H6" }
                path { d: "M15 11h3" }
                path { d: "M15 13h3" }
            }
        },
        Screen::CncProfiles => rsx! {
            // CNC machine: a portal/gantry frame with a spindle head — the hardware.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M4 20h16" }
                path { d: "M6 20V8h12v12" }
                rect { x: "10", y: "8", width: "4", height: "5", rx: "0.8" }
                path { d: "M12 13v2.5" }
            }
        },
        Screen::FixtureProfiles => rsx! {
            // Vise: two jaws clamping a board between them — holding the work.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "2.5", y: "8", width: "4", height: "8", rx: "1" }
                rect { x: "17.5", y: "8", width: "4", height: "8", rx: "1" }
                rect { x: "6.5", y: "10", width: "11", height: "4", rx: "0.6" }
            }
        },
        Screen::MachiningProfiles => rsx! {
            // A cutting bit entering a workpiece surface — the machining operation.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "10", y: "3", width: "4", height: "8", rx: "1" }
                path { d: "M10 11l2 5 2-5" }
                path { d: "M3 14h18" }
            }
        },
        Screen::Stock => rsx! {
            // A drawer cabinet — the tool inventory.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "4", y: "4", width: "16", height: "16", rx: "1.5" }
                path { d: "M4 9.3h16" }
                path { d: "M4 14.6h16" }
                path { d: "M10.5 6.7h3" }
                path { d: "M10.5 11.9h3" }
                path { d: "M10.5 17.2h3" }
            }
        },
        Screen::ToolsetProfiles => rsx! {
            // A rack rail with three tool bits hanging from it — the loaded tool set.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M4 6h16" }
                path { d: "M8 6v6" }
                path { d: "M6.6 12L8 15l1.4-3" }
                path { d: "M12 6v6" }
                path { d: "M10.6 12L12 15l1.4-3" }
                path { d: "M16 6v6" }
                path { d: "M14.6 12L16 15l1.4-3" }
            }
        },
        Screen::Catalog => rsx! {
            // An open book — the reference catalog.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                path { d: "M12 6C9 4.5 6 4.5 4 6v12c2-1.5 5-1.5 8 0" }
                path { d: "M12 6c3-1.5 6-1.5 8 0v12c-2-1.5-5-1.5-8 0" }
                path { d: "M12 6v12" }
            }
        },
    }
}

#[component]
pub fn EventNotifications(state: Signal<crate::runtime::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let visible_events = snapshot
        .events
        .iter()
        .rev()
        .take(4)
        .cloned()
        .collect::<Vec<_>>();

    if visible_events.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "event-toast-stack",
            for event in visible_events.into_iter() {
                div { key: "{event.id}", class: "event-toast", "{event.message}" }
            }
        }
    }
}

#[component]
pub fn StatusBar(state: Signal<crate::runtime::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let connected = snapshot.kicad_status != "not connected";
    let board_label = snapshot
        .board
        .as_ref()
        .map(|board| format!("{} holes · {} edges", board.holes.len(), board.edge_shapes.len()))
        .unwrap_or_else(|| "No board".to_string());
    let generation_label = match snapshot.generation_state {
        GenerationState::Running => "Generating GCode…".to_string(),
        GenerationState::Failed => "Generation failed".to_string(),
        GenerationState::Idle => {
            if snapshot.gcode.is_empty() {
                "No program".to_string()
            } else if snapshot.gcode_modified {
                "Program (edited)".to_string()
            } else {
                "Program ready".to_string()
            }
        }
    };

    rsx! {
        footer { class: "shell-statusbar",
            span { class: if connected { "status-connection ok" } else { "status-connection err" },
                "KiCad: {snapshot.kicad_status}"
            }
            span { class: "status-meta", "{board_label}" }
            span { class: "status-meta", "{generation_label}" }
        }
    }
}


