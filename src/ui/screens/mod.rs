use dioxus::prelude::*;

use crate::ui::navigation::*;
use super::theme::APP_STYLE;
use crate::runtime::{ctx_snapshot, with_ctx_mut, MIN_JOB_PIN_WIDTH};

mod about;
mod cnc;
mod catalog;
mod fixture;
mod logs;
mod manual;
mod profile_manager;
mod profiles_common;
mod job;
mod machining;
mod save_program;
mod settings;
mod shell;
mod stock;
mod toolset;

use about::AboutScreen;
use cnc::CncScreen;
use catalog::CatalogScreen;
use fixture::FixtureProfilesScreen;
use logs::LogsScreen;
use manual::ManualScreen;
use job::{JobScreen, JobViewPanel};
use machining::MachiningProfilesScreen;
use settings::SettingsDialog;
use shell::{
    AppTopBar, DiagnosticsBanner, EventNotifications, NavigationRail, StatusBar, UpdateBanner,
};
use stock::StockScreen;
use toolset::ToolsetProfilesScreen;

pub fn mutate_ctx<R>(mut state: Signal<crate::runtime::AppCtx>, f: impl FnOnce(&mut crate::runtime::AppCtx) -> R) -> R {
    let result = with_ctx_mut(f);
    state.set(ctx_snapshot());
    result
}

#[component]
pub fn AppRoot() -> Element {
    let mut state = use_signal(ctx_snapshot);
    let show_error_details = use_signal(|| false);
    // Owned here rather than in `AppTopBar`, where the cog that sets it lives — see the
    // note beside the dialog itself at the foot of the shell.
    let mut settings_open = use_signal(|| false);

    // Remember the window's size and maximized state for the next launch.
    crate::ui::window_state::use_window_geometry();

    // The bridge from the datastore to the legacy snapshot every view reads.
    //
    // AppData writes bump the store revision; nothing else pushes those writes into
    // `state`. Mounted here, at the root, it runs for every store write regardless of
    // which screen is on show — which is the point: the six per-screen copies it replaced
    // each refreshed one realm, so a view reading two of them (the Stock screen's ATC
    // column resolves a rack from the machines *and* the toolsets) could see one half
    // current and the other half as it was when its editor was last open.
    //
    // The effect re-runs on the revision alone; `state.set` only writes, so there is no
    // feedback loop.
    use_effect(move || {
        let _ = crate::ui::bindings::data_revision();
        crate::ui::bindings::refresh_legacy_projections();
        state.set(ctx_snapshot());
    });

    // Keep the platform's own widgets on the same side of light/dark as the stylesheet.
    // Re-runs whenever the theme changes, because it reads the signal — see
    // `apply_platform_theme` for what is not drawn by the webview and why it matters.
    use_effect(move || {
        crate::ui::apply_platform_theme(state.read().theme == Theme::Dark);
    });

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
            AppTopBar { state, settings_open }

            UpdateBanner { state }

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
                                .set(next.max(MIN_JOB_PIN_WIDTH) as f64);
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
                                Screen::Manual => rsx! {
                                    ManualScreen { state }
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

            // Last child of `.app-shell`, and nowhere deeper. `.wizard-overlay` is
            // `position: absolute; inset: 0`, so it fills the nearest positioned
            // ancestor — which is `.app-shell` — and so covers the rail and status bar
            // the way a modal should. Inside `.screen-host` (`overflow: auto`) it would
            // be clipped and would scroll with the screen; inside `AppTopBar` it would
            // work only for as long as `.shell-topbar` never gains a `position` of its
            // own, which is an invariant nothing states or checks.
            //
            // Mounted only while open: the dialog's KiCad probe walks two directory
            // trees and enumerates processes, which is not something to keep warm behind
            // a hidden element.
            if *settings_open.read() {
                SettingsDialog { state, on_close: move |_| settings_open.set(false) }
            }
        }
    }
}



#[cfg(test)]
mod projection_bridge_tests {
    /// Every screen, plus the root that owns the bridge. Compiled in for the same reason
    /// the dialog test below compiles its sources in: the invariant is about what the
    /// shipping code *is*, and nothing observable at runtime can stand in for it.
    const SOURCES: &[(&str, &str)] = &[
        ("mod.rs", include_str!("mod.rs")),
        ("cnc.rs", include_str!("cnc.rs")),
        ("fixture.rs", include_str!("fixture.rs")),
        ("toolset.rs", include_str!("toolset.rs")),
        ("machining.rs", include_str!("machining.rs")),
        ("stock.rs", include_str!("stock.rs")),
        ("catalog.rs", include_str!("catalog.rs")),
        ("job/mod.rs", include_str!("job/mod.rs")),
    ];

    const BINDINGS: &str = include_str!("../bindings.rs");

    /// The bridge refreshes **every** realm, not the one its caller happens to care about.
    ///
    /// This is the whole of the bug it replaced. Six screens each carried a copy of the
    /// effect and each refreshed one realm, so what the Job views held depended on which
    /// editor had last been open — and the Stock screen's ATC column, which resolves a
    /// rack against the machines *and* the toolsets, could read one of them current and
    /// the other as it was an hour ago. A realm dropped from this function reinstates that
    /// silently, so it is asserted rather than trusted.
    #[test]
    fn the_bridge_refreshes_every_realm() {
        let body = BINDINGS
            .split_once("pub fn refresh_legacy_projections()")
            .expect("the bridge must exist")
            .1;
        for realm in [
            "refresh_machines",
            "refresh_fixtures",
            "refresh_toolsets",
            "refresh_tools",
            "refresh_process_profiles",
        ] {
            assert!(
                body.contains(realm),
                "refresh_legacy_projections does not call `{realm}`. Every legacy \
                 projection is refreshed together or the views disagree with each other."
            );
        }
    }

    /// No screen may install a projection bridge of its own.
    ///
    /// One bridge, at the root, running for every store write regardless of what is on
    /// screen. A per-screen copy works perfectly while that screen is the one being used,
    /// which is exactly why the fault it causes elsewhere took so long to see.
    #[test]
    fn no_screen_carries_its_own_bridge() {
        for (name, source) in SOURCES {
            if *name == "mod.rs" {
                continue; // the root, where the one bridge lives
            }
            for (line_no, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or(line);
                if code.contains("refresh_legacy_") {
                    panic!(
                        "{name}:{} refreshes a legacy projection. That is the root's job \
                         (`AppRoot`'s `refresh_legacy_projections` effect) — a screen-local \
                         copy refreshes one realm and only while that screen is mounted.\n    {}",
                        line_no + 1,
                        line.trim()
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod dialog_safety_tests {
    /// Every screen that opens a native dialog. Compiled in so the test reads the shipping
    /// source rather than walking the filesystem at test time.
    const SCREEN_SOURCES: &[(&str, &str)] = &[
        ("save_program.rs", include_str!("save_program.rs")),
        ("catalog.rs", include_str!("catalog.rs")),
        ("profile_manager.rs", include_str!("profile_manager.rs")),
        ("machining.rs", include_str!("machining.rs")),
        ("toolset.rs", include_str!("toolset.rs")),
        ("profiles_common.rs", include_str!("profiles_common.rs")),
        ("settings.rs", include_str!("settings.rs")),
    ];

    /// No screen may open a **blocking** native dialog.
    ///
    /// `rfd::FileDialog` and `rfd::MessageDialog` (the non-`Async` types) run the
    /// platform's own modal message pump. Called from a Dioxus event handler — which is
    /// the only place a screen ever calls one — that pump re-enters tao's event loop and
    /// `VirtualDom::render_immediate` while dioxus-core still holds a borrow of the
    /// element arena for the event being dispatched. The result is
    /// `RefCell already borrowed`, then a second panic as the first unwinds through the
    /// dialog component's props: the application aborts, mid-save.
    ///
    /// It cost a crash report to find, and the fix is invisible in review — `FileDialog`
    /// and `AsyncFileDialog` differ by five characters, and the blocking one works
    /// perfectly every time it is tried by hand on a fast machine. Nothing else can catch
    /// a re-entrancy fault in a unit test, so this reads the source instead.
    #[test]
    fn no_blocking_native_dialogs_in_the_screens() {
        for (name, source) in SCREEN_SOURCES {
            for (line_no, line) in source.lines().enumerate() {
                let code = line.split("//").next().unwrap_or(line);
                for blocking in ["FileDialog::new()", "MessageDialog::new()"] {
                    let Some(at) = code.find(blocking) else { continue };
                    // `AsyncFileDialog::new()` contains `FileDialog::new()`; the prefix is
                    // what tells the two apart.
                    if code[..at].ends_with("Async") {
                        continue;
                    }
                    panic!(
                        "{name}:{} opens a blocking `{blocking}`. Its modal pump re-enters \
                         the Dioxus event loop and panics the element arena. Use \
                         `rfd::Async…` driven by `spawn`, or the helpers in \
                         `profiles_common` (`confirm`, `pick_import_file`, \
                         `pick_export_file`).\n    {}",
                        line_no + 1,
                        line.trim()
                    );
                }
            }
        }
    }
}
