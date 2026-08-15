use dioxus::prelude::*;
use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;

use crate::data::model::*;
use crate::ui::navigation::*;
use crate::runtime::AppError;
use crate::runtime::{UiCommand, apply_ui_command, ctx_snapshot, with_ctx_mut};
use super::save_program::SaveProgramButton;

#[component]
pub fn AppTopBar(
    state: Signal<crate::runtime::AppCtx>,
    settings_open: Signal<bool>,
) -> Element {
    let mut settings_open = settings_open;
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
    // The pill reflects the generated program's availability — the thing a user
    // actually waits on. Errors/warnings are surfaced by the DiagnosticsBanner. A
    // blocking error (e.g. no tooling solution) means the job cannot be machined, so
    // the program is not "ready" regardless of any stale output.
    let has_blocking_error = snapshot.errors.iter().any(|error| error.is_error);
    // The readiness gate (orchestration) is the authority on whether the job *can*
    // be machined: it captures preconditions the pill can't infer from
    // `generation_state` alone — no board loaded, incomplete profiles, open
    // contours. A closed gate must never read as "Program ready", even when stale or
    // placeholder gcode is still in view. Absent (defensive) ⇒ treat as not ready.
    let is_ready = snapshot
        .status
        .get(crate::runtime::STATUS_KEY_GENERATION_READINESS)
        .map(|value| value == "true")
        .unwrap_or(false);
    let not_ready = has_blocking_error || !is_ready;
    // A job is N programs, one per step. The one-step wording is exactly what it always
    // was — a single-step job must show no trace of the step machinery — and only a
    // multi-step job counts out loud.
    let total = snapshot.programs.len();
    let ready_count = snapshot.programs.iter().filter(|s| s.program().is_some()).count();
    let failed: Vec<String> = snapshot
        .programs
        .iter()
        .filter(|s| s.failure().is_some())
        .map(|s| format!("Step {}: {}", s.index + 1, s.failure().unwrap_or_default()))
        .collect();
    let status_label = match snapshot.generation_state {
        GenerationState::Running => "Generating…".to_string(),
        GenerationState::Failed => "Generation failed".to_string(),
        _ if not_ready => "Not ready".to_string(),
        GenerationState::Idle if ready_count == 0 => "No program".to_string(),
        // Partly failed is styled as an error even though saving is possible: a job with
        // a step that produced nothing is not one to walk away from.
        GenerationState::Idle if !failed.is_empty() => {
            format!("{} of {total} steps failed", failed.len())
        }
        GenerationState::Idle if total > 1 => format!("{total} programs ready"),
        GenerationState::Idle => "Program ready".to_string(),
    };
    let status_detail = failed.join(" · ");

    rsx! {
        header { class: "shell-topbar",
            button {
                class: "brand-block",
                r#type: "button",
                title: "About K2G",
                "aria-label": "About K2G",
                onclick: move |_| super::mutate_ctx(state, |s| s.select_screen(Screen::About)),
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
                        GenerationState::Failed => "status-pill status-err",
                        _ if not_ready => "status-pill status-err",
                        GenerationState::Idle if ready_count == 0 => "status-pill status-warn",
                        GenerationState::Idle if !failed.is_empty() => "status-pill status-err",
                        GenerationState::Idle => "status-pill status-ok",
                    },
                    // Which steps failed, without spending pill width on them.
                    title: status_detail,
                    "{status_label}"
                }

                // Immediately right of the pill: the pill says the program is ready, and
                // this is what you do about it.
                SaveProgramButton { state }

                // Application preferences, including the palette that used to have its
                // own button here. Icon-only, so it carries the label a sighted user
                // gets from the tooltip and a screen reader gets from `aria-label`.
                button {
                    class: "icon-button topbar-settings-btn",
                    r#type: "button",
                    title: "Settings",
                    "aria-label": "Settings",
                    "aria-haspopup": "dialog",
                    "aria-expanded": if *settings_open.read() { "true" } else { "false" },
                    onclick: move |_| settings_open.set(true),
                    SettingsCogIcon {}
                }
            }
        }
    }
}

/// The Settings cog.
///
/// A real gear rather than the sliders glyph the rail used, because up here it has no
/// label beside it: three sliders with no word next to them read as a filter or an
/// equaliser, and the cog is the one shape every operator already takes to mean
/// "preferences".
///
/// Generated rather than placed by hand — a gear will not come out of the round-numbered
/// primitives the other icons are built from. Centre 12,12; eight teeth on a 45° pitch;
/// tips on r=9.9 and roots on r=7.5; each tooth 20° wide at the tip and 32° at the root,
/// with the valleys between them arcs of the root circle. Eight teeth is the fewest that
/// still reads as a gear and the most that survives the size — twelve blur into a fuzzy
/// ring. The hub is a separate circle so the centre stays open; closed, the whole thing
/// is a blob at a glance.
#[component]
fn SettingsCogIcon() -> Element {
    rsx! {
        svg {
            class: "topbar-settings-svg",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            path { d: "M19.21 9.93 L21.75 10.28 L21.75 13.72 L19.21 14.07 A7.5 7.5 0 0 1 18.56 15.64 L20.11 17.68 L17.68 20.11 L15.64 18.56 A7.5 7.5 0 0 1 14.07 19.21 L13.72 21.75 L10.28 21.75 L9.93 19.21 A7.5 7.5 0 0 1 8.36 18.56 L6.32 20.11 L3.89 17.68 L5.44 15.64 A7.5 7.5 0 0 1 4.79 14.07 L2.25 13.72 L2.25 10.28 L4.79 9.93 A7.5 7.5 0 0 1 5.44 8.36 L3.89 6.32 L6.32 3.89 L8.36 5.44 A7.5 7.5 0 0 1 9.93 4.79 L10.28 2.25 L13.72 2.25 L14.07 4.79 A7.5 7.5 0 0 1 15.64 5.44 L17.68 3.89 L20.11 6.32 L18.56 8.36 A7.5 7.5 0 0 1 19.21 9.93 Z" }
            circle { cx: "12", cy: "12", r: "3.4" }
        }
    }
}

/// Re-acquire the reachable KiCad's board (recovering a connection made after
/// startup) and update the status. Setting a changed board re-stitches once and
/// triggers regeneration (see `sync_after_mutation`).
fn do_refresh(state: Signal<crate::runtime::AppCtx>) {
    let acquired = crate::runtime::acquire_board();
    super::mutate_ctx(state, |ctx| {
        ctx.kicad_status = acquired.status;
        ctx.board = acquired.board;
        // Replaced together with the board, never independently: copper from one revision
        // of a file drawn under the outline of another is a picture of no board at all.
        ctx.copper = acquired.copper;
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

/// Announces an available release, with the four choices EU CRA Annex I (2)(c) asks
/// for: take it, postpone it, skip this one, or stop being asked.
///
/// Deliberately a banner rather than a modal. k2g may be mid-job with a spindle
/// warm, and a dialog that steals focus to talk about software versions is the wrong
/// interruption. Nothing here downloads or installs without the explicit click.
#[component]
pub fn UpdateBanner(state: Signal<crate::runtime::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let Some(update) = snapshot.available_update.clone() else {
        return rsx! {};
    };
    let installing = snapshot.update_installing;

    let current = env!("CARGO_PKG_VERSION");
    let version = update.version.clone();
    // First non-empty line of the release notes. The full text is on the release
    // page, and a banner is not the place to render markdown.
    let headline = update
        .notes
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect::<String>();

    let for_install = update.clone();
    let skip_version = update.version.clone();

    rsx! {
        div { class: "diag-banner-wrap",
            div { class: "update-banner",
                div { class: "diag-banner-main",
                    span { class: "update-banner-dot" }
                    div { class: "diag-banner-copy",
                        div { class: "diag-banner-title", "k2g {version} is available" }
                        div { class: "diag-banner-subtitle",
                            if installing {
                                "Downloading and checking the signature…"
                            } else if headline.is_empty() {
                                "You are running {current}. The installer is signature-checked before it runs."
                            } else {
                                "{headline}"
                            }
                        }
                    }
                }

                div { class: "update-banner-actions",
                    button {
                        class: "text-button",
                        disabled: installing,
                        onclick: move |_| {
                            super::mutate_ctx(state, |s| {
                                s.app.update_installing = true;
                                s.log_event("Downloading the k2g update…");
                            });
                            let mut state = state;
                            crate::runtime::update::start_install(
                                for_install.clone(),
                                move |outcome| {
                                    match outcome {
                                        Ok(()) => {
                                            crate::runtime::with_ctx_mut(|ctx| {
                                                ctx.log_event(
                                                    "Installer verified and started — close k2g to let it finish.",
                                                );
                                            });
                                        }
                                        Err(message) => {
                                            crate::runtime::with_ctx_mut(|ctx| {
                                                ctx.app.update_installing = false;
                                                ctx.log_event(format!("Update failed: {message}"));
                                            });
                                        }
                                    }
                                    crate::runtime::wake_ui();
                                },
                            );
                            state.set(crate::runtime::ctx_snapshot());
                        },
                        if installing { "Installing…" } else { "Install" }
                    }
                    button {
                        class: "text-button",
                        disabled: installing,
                        onclick: move |_| {
                            super::mutate_ctx(state, |s| {
                                s.postpone_update(crate::runtime::update::POSTPONE_DAYS);
                                s.app.available_update = None;
                            });
                        },
                        "Remind me later"
                    }
                    button {
                        class: "text-button",
                        disabled: installing,
                        onclick: move |_| {
                            let version = skip_version.clone();
                            super::mutate_ctx(state, |s| {
                                s.skip_update_version(version);
                                s.app.available_update = None;
                            });
                        },
                        "Skip this version"
                    }
                    button {
                        class: "text-button",
                        disabled: installing,
                        onclick: move |_| {
                            super::mutate_ctx(state, |s| {
                                s.set_update_check_enabled(false);
                                s.app.available_update = None;
                                s.log_event("Update checks are off. k2g will make no network requests.");
                            });
                        },
                        "Turn off update checks"
                    }
                }
            }
        }
    }
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
    // A count alone ("1 errors, 0 warnings") says something is wrong but not what, so the
    // operator has to click through to find out — and the commonest case by far is a single
    // entry, where there is nothing to summarise. So one entry states itself; several fall
    // back to the run status, which is the only honest thing to say about a mixed set.
    let status_text = match (errors.as_slice(), generation_state) {
        ([only], _) => only.details.clone().unwrap_or_else(|| only.message.clone()),
        (_, GenerationState::Running) => "Generating…".to_string(),
        (_, GenerationState::Failed) => "Generation failed".to_string(),
        (_, GenerationState::Idle) => "Diagnostics available".to_string(),
    };
    let banner_title = match errors.as_slice() {
        [only] => only.message.clone(),
        _ => format!("{error_count} errors, {warning_count} warnings"),
    };

    rsx! {
        div { class: "diag-banner-wrap",
            div { class: banner_class,
                div { class: "diag-banner-main",
                    span { class: "diag-banner-dot" }
                    div { class: "diag-banner-copy",
                        div { class: "diag-banner-title", "{banner_title}" }
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

/// The rail's class list for one nav entry.
///
/// `active` is "this screen is on screen now"; `is-pinned` is "the Job view is docked
/// beside whatever screen you go to". They are independent — the Job entry can carry
/// either, both, or neither — so the styles must not be the same one, or a pinned Job
/// would read as the current screen (see the rail CSS in `theme.rs`).
fn rail_button_class(screen: Screen, selected: Screen, job_pinned: bool) -> String {
    let mut classes = String::from("rail-button");
    if screen == selected {
        classes.push_str(" active");
    }
    if screen == Screen::Job && job_pinned {
        classes.push_str(" is-pinned");
    }
    classes
}

#[component]
pub fn NavigationRail(state: Signal<crate::runtime::AppCtx>) -> Element {
    let snapshot = state.read().clone();
    let selected = snapshot.selected_screen;
    // The pin lives here rather than inside the Job view because turning it *on* is
    // the common act, and it was previously only reachable by first navigating to the
    // Job screen — i.e. away from the screen the dock was wanted on.
    let pinned = snapshot.job_view_pinned;
    let pin_title = if pinned {
        "Unpin — stop showing the Job view beside the profile and inventory screens"
    } else {
        "Pin — keep the Job view visible beside the profile and inventory screens"
    };
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
        None,
        // The manual opens the housekeeping group rather than trailing it: it is what an
        // operator reaches for while working, where Logs and About are looked up once.
        Some(Screen::Manual),
        Some(Screen::Logs),
        Some(Screen::About),
    ];

    rsx! {
        aside { class: "shell-rail",
            for (idx , item) in nav_items.iter().enumerate() {
                if let Some(screen) = *item {
                    // A row, not a bare button: the pin has to be a *sibling* of the
                    // nav button (a button inside a button is invalid HTML and the
                    // inner click would not be reachable).
                    div { key: "{screen.key()}", class: "rail-item",
                        button {
                            class: rail_button_class(screen, selected, pinned),
                            onclick: move |_| super::mutate_ctx(state, |s| s.select_screen(screen)),
                            span { class: "rail-button-content",
                                span { class: "rail-button-icon", {rail_icon(screen)} }
                                span { class: "rail-button-text", "{screen.label()}" }
                            }
                        }
                        if screen == Screen::Job {
                            button {
                                class: if pinned { "rail-pin-toggle active" } else { "rail-pin-toggle" },
                                r#type: "button",
                                title: "{pin_title}",
                                "aria-label": "{pin_title}",
                                "aria-pressed": if pinned { "true" } else { "false" },
                                onclick: move |_| super::mutate_ctx(state, |s| s.toggle_job_view_pinned()),
                                "📌"
                            }
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
        Screen::Manual => rsx! {
            // A closed book with a bookmark hanging out of it — the manual.
            //
            // Deliberately not another open book: Catalog is one, two rows up, and at
            // 20 px the difference between two open books is nothing. A filled cover with
            // a spine and a ribbon is a different silhouette at a glance, which is the
            // only thing an icon this size has to achieve.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "4", y: "3", width: "16", height: "18", rx: "2" }
                path { d: "M8 3v18" }
                path { d: "M13 3v7l2-1.6 2 1.6V3" }
            }
        },
        Screen::Logs => rsx! {
            // Lines of text on a page — the log stream.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                rect { x: "4", y: "3", width: "16", height: "18", rx: "2" }
                path { d: "M8 8h8" }
                path { d: "M8 12h8" }
                path { d: "M8 16h5" }
            }
        },
        Screen::About => rsx! {
            // An info circle — application details.
            svg {
                class: "rail-icon-svg",
                view_box: "0 0 24 24",
                "aria-hidden": "true",
                circle { cx: "12", cy: "12", r: "9" }
                path { d: "M12 11v5" }
                circle { cx: "12", cy: "7.5", r: "0.6" }
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
    // Only a resolved version string means KiCad answered; "not connected" and
    // "not responding" are both red.
    let connected = snapshot
        .kicad_status
        .starts_with(crate::runtime::KICAD_STATUS_OK_PREFIX);

    // Program availability now lives in the top-bar pill; board geometry lives in
    // the Board view. The status bar owns the KiCad connection state.
    rsx! {
        footer { class: "shell-statusbar",
            span { class: if connected { "status-connection ok" } else { "status-connection err" },
                "KiCad: {snapshot.kicad_status}"
            }
        }
    }
}


